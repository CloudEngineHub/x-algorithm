use crate::hydration::Cause;
use strum::VariantArray;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stake {
    Exists,
    MustHave,
    BestEffort,
    Absent(Tier),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    MustHave,
    BestEffort,
}

impl Stake {
    pub(crate) const fn tier(self) -> Tier {
        match self {
            Stake::Exists | Stake::MustHave => Tier::MustHave,
            Stake::BestEffort => Tier::BestEffort,
            Stake::Absent(tier) => tier,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Answer {
    Found,
    Partial,
    NotFound,
    Failed,
    Unanswered,
    Unclaimed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    Value,
    Absent,
    Fails,
    Unresolves(Cause),
}

impl Effect {
    pub(super) const fn fails_node(self) -> bool {
        matches!(self, Effect::Fails | Effect::Unresolves(Cause::Failed))
    }
}

pub(super) const fn classify(stake: Stake, answer: Answer) -> Effect {
    match (stake, answer) {
        (_, Answer::Found) | (Stake::Absent(_), Answer::Partial) => Effect::Value,
        (Stake::Exists, Answer::NotFound) => Effect::Unresolves(Cause::NotFound),
        (Stake::Exists, Answer::Failed | Answer::Unanswered | Answer::Unclaimed) => {
            Effect::Unresolves(Cause::Failed)
        }
        (_, Answer::NotFound) | (Stake::Absent(_), Answer::Failed | Answer::Unanswered) => {
            Effect::Absent
        }
        (Stake::Exists | Stake::MustHave | Stake::BestEffort, Answer::Partial)
        | (Stake::MustHave | Stake::BestEffort, Answer::Failed | Answer::Unanswered)
        | (_, Answer::Unclaimed) => Effect::Fails,
    }
}

pub(super) const fn claimed_answer_fails(stake: Stake, answer: Answer) -> bool {
    !matches!(answer, Answer::Unclaimed) && classify(stake, answer).fails_node()
}

pub(super) struct Row {
    pub(super) decides_existence: bool,
    pub(super) batch: Batch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Batch {
    Unrecorded,
    Keys,
    Requested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, VariantArray)]
pub(crate) enum Source {
    TesPureCoreData,
    TesTweet,
    TesConversationControl,
    TesQuotedTweet,
    SafetyLabels,
    GizmoduckViewer,
    GizmoduckAuthor,
    Flock,
    FlockUserPair,
    ViewerCountry,
    Wingman,
    CommunityModeration,
    CommunityModerator,
    CommunityViewerRemoved,
    ArticleLifecycle,
    ArticleMediaEntities,
    ArticleCoverMedia,
    TrustedFriends,
    UserLocation,
    GizmoduckActor,
    ActorEmails,
    ActorPhones,
}

impl Source {
    pub(super) const fn row(self) -> Row {
        match self {
            Source::TesPureCoreData => Row {
                decides_existence: true,
                batch: Batch::Keys,
            },
            Source::TesTweet => Row {
                decides_existence: true,
                batch: Batch::Unrecorded,
            },
            Source::TesConversationControl => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::TesQuotedTweet => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::SafetyLabels => Row {
                decides_existence: false,
                batch: Batch::Requested,
            },
            Source::GizmoduckViewer => Row {
                decides_existence: false,
                batch: Batch::Unrecorded,
            },
            Source::GizmoduckAuthor => Row {
                decides_existence: true,
                batch: Batch::Keys,
            },
            Source::Flock => Row {
                decides_existence: false,
                batch: Batch::Requested,
            },
            Source::FlockUserPair => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::ViewerCountry => Row {
                decides_existence: false,
                batch: Batch::Unrecorded,
            },
            Source::Wingman => Row {
                decides_existence: false,
                batch: Batch::Requested,
            },
            Source::CommunityModeration => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::CommunityModerator => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::CommunityViewerRemoved => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::ArticleLifecycle => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::ArticleMediaEntities => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::ArticleCoverMedia => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::TrustedFriends => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
            Source::UserLocation => Row {
                decides_existence: false,
                batch: Batch::Unrecorded,
            },
            Source::GizmoduckActor => Row {
                decides_existence: true,
                batch: Batch::Keys,
            },
            Source::ActorEmails | Source::ActorPhones => Row {
                decides_existence: false,
                batch: Batch::Keys,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_stake_classifies_each_answer() {
        use Answer::{Failed, Found, NotFound, Partial, Unanswered, Unclaimed};
        use Effect::{Absent, Fails, Unresolves, Value};
        let answers = [Found, Partial, NotFound, Failed, Unanswered, Unclaimed];
        let unresolves = Unresolves(Cause::Failed);
        let not_found = Unresolves(Cause::NotFound);
        let absent = [Value, Value, Absent, Absent, Absent, Fails];
        let table = [
            (
                Stake::Exists,
                [Value, Fails, not_found, unresolves, unresolves, unresolves],
            ),
            (Stake::MustHave, [Value, Fails, Absent, Fails, Fails, Fails]),
            (
                Stake::BestEffort,
                [Value, Fails, Absent, Fails, Fails, Fails],
            ),
            (Stake::Absent(Tier::MustHave), absent),
            (Stake::Absent(Tier::BestEffort), absent),
        ];
        for (stake, effects) in table {
            for (answer, effect) in answers.into_iter().zip(effects) {
                assert_eq!(classify(stake, answer), effect, "{stake:?} {answer:?}");
            }
        }
    }
}
