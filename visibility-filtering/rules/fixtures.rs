use crate::models::{
    AuthorFeatures, AuthorLabel, ConversationControlFeatures, Decided, HydratedTweetCandidate,
    LimitedEngagement, LimitedEngagementReason, MediaInterstitial, SafetyLabelMap, SafetyLabelType,
    TweetFeatures, Verdict, Viewer, ViewerAuthorRelationship, ViewerBlockedBy, ViewerFeatures,
    ViewerProfile, Withholding,
};
use std::collections::HashSet;
use xai_core_entities::entities::{ConversationControl, ConversationControlArm};
use xai_visibility_filtering::models::FilteredReason;
use xai_x_thrift::action::InterstitialReason;

const TWEET_ID: u64 = 1;
pub(super) const AUTHOR_ID: u64 = 100;
pub(crate) const VIEWER_ID: u64 = 999;

pub(crate) fn allow() -> Verdict {
    Verdict::Shown {
        media: None,
        engagement: None,
    }
}

pub(crate) fn dropped(reason: FilteredReason, by: &'static str) -> Verdict {
    Verdict::Withheld(Decided {
        value: Withholding::Drop(reason),
        by,
    })
}

pub(crate) fn blurred(reason: InterstitialReason, by: &'static str) -> Verdict {
    Verdict::Shown {
        media: Some(Decided {
            value: MediaInterstitial {
                legacy: FilteredReason::ContainNsfwMedia,
                reason,
            },
            by,
        }),
        engagement: None,
    }
}

pub(crate) fn limited(reason: LimitedEngagementReason, by: &'static str) -> Verdict {
    Verdict::Shown {
        media: None,
        engagement: Some(Decided {
            value: LimitedEngagement(reason),
            by,
        }),
    }
}

pub(crate) fn blurred_and_limited(blur: Verdict, limit: Verdict) -> Verdict {
    match (blur, limit) {
        (Verdict::Shown { media, .. }, Verdict::Shown { engagement, .. }) => {
            Verdict::Shown { media, engagement }
        }
        (blur, limit) => panic!("expected two Shown verdicts, got {blur:?} and {limit:?}"),
    }
}

pub(crate) fn viewer(id: u64) -> ViewerFeatures {
    ViewerFeatures {
        viewer: Viewer::LoggedIn {
            id,
            profile: ViewerProfile::default(),
        },
        ..Default::default()
    }
}

pub(crate) fn viewer_with_profile(profile: ViewerProfile) -> ViewerFeatures {
    ViewerFeatures {
        viewer: Viewer::LoggedIn {
            id: VIEWER_ID,
            profile,
        },
        ..Default::default()
    }
}

pub(crate) fn author_viewer() -> ViewerFeatures {
    viewer(AUTHOR_ID)
}

pub(crate) fn logged_out_viewer() -> ViewerFeatures {
    ViewerFeatures {
        viewer: Viewer::LoggedOut,
        ..Default::default()
    }
}

pub(crate) fn sensitive_opt_in_viewer() -> ViewerFeatures {
    viewer_with_profile(ViewerProfile {
        allows_sensitive_media: true,
        ..ViewerProfile::default()
    })
}

pub(super) fn conversation_control(
    arm: ConversationControlArm,
    root_author_id: u64,
) -> ConversationControlFeatures {
    ConversationControlFeatures {
        control: ConversationControl {
            arm,
            conversation_tweet_author_id: root_author_id,
            invited_user_ids: vec![],
            invite_via_mention: None,
            allowed_country_codes: vec![],
        },
        root_author_follows_viewer: None,
        viewer_super_follows_root_author: None,
        viewer_country: None,
    }
}

pub(crate) fn candidate() -> CandidateBuilder {
    CandidateBuilder {
        candidate: HydratedTweetCandidate {
            tweet_id: TWEET_ID,
            author_id: AUTHOR_ID,
            ..Default::default()
        },
        labels: HashSet::new(),
    }
}

pub(crate) struct CandidateBuilder {
    candidate: HydratedTweetCandidate,
    labels: HashSet<SafetyLabelType>,
}

impl CandidateBuilder {
    pub(crate) fn tweet_id(mut self, id: u64) -> Self {
        self.candidate.tweet_id = id;
        self
    }

    pub(crate) fn with_label(mut self, label: SafetyLabelType) -> Self {
        self.labels.insert(label);
        self
    }

    pub(crate) fn with_author_user_label(mut self, label: AuthorLabel) -> Self {
        self.candidate.author_labels.insert(label);
        self
    }

    pub(crate) fn with_tweet_features(mut self, features: TweetFeatures) -> Self {
        self.candidate.tweet_features = features;
        self
    }

    pub(crate) fn with_author_features(mut self, features: AuthorFeatures) -> Self {
        self.candidate.author_features = features;
        self
    }

    pub(crate) fn with_relationship(mut self, relationship: ViewerAuthorRelationship) -> Self {
        self.candidate.relationship = relationship;
        self
    }

    pub(crate) fn blocked_by(mut self, blocked_by: ViewerBlockedBy) -> Self {
        self.candidate.blocked_by = blocked_by;
        self
    }

    pub(crate) fn with_media(mut self) -> Self {
        self.candidate.tweet_features.media.has_media = true;
        self
    }

    pub(crate) fn with_conversation_control(
        mut self,
        features: ConversationControlFeatures,
    ) -> Self {
        self.candidate.conversation_control = Some(features);
        self
    }

    pub(crate) fn retweet_of(mut self, source_tweet_id: u64) -> Self {
        self.candidate.tweet_features.source_tweet_id = Some(source_tweet_id);
        self
    }

    pub(crate) fn build(self) -> HydratedTweetCandidate {
        let mut candidate = self.candidate;
        if !self.labels.is_empty() {
            candidate.safety_labels = SafetyLabelMap::new(self.labels);
        }
        candidate
    }
}
