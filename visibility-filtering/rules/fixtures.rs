use crate::models::{
    AuthorFeatures, AuthorLabel, Decided, HydratedTweetCandidate, SafetyLabelMap, SafetyLabelType,
    TweetFeatures, Verdict, Viewer, ViewerAuthorRelationship, ViewerFeatures, Withholding,
};
use crate::rules::registry::Policy;
use crate::rules::rule_spec::RuleClause;
use crate::rules::test_context;
use std::collections::HashSet;
use std::slice::from_ref;
use xai_visibility_filtering::models::FilteredReason;

const TWEET_ID: u64 = 1;
const AUTHOR_ID: u64 = 100;
pub(crate) const VIEWER_ID: u64 = 999;

pub(super) fn clauses_verdict(
    clauses: &[RuleClause],
    viewer: &ViewerFeatures,
    candidate: &HydratedTweetCandidate,
) -> Verdict {
    Policy::new(&[clauses]).evaluate(&test_context(viewer, candidate))
}

pub(super) fn assert_clauses_drop(
    clauses: &[RuleClause],
    viewer: &ViewerFeatures,
    candidate: &HydratedTweetCandidate,
    expected: &FilteredReason,
) {
    let verdict = clauses_verdict(clauses, viewer, candidate);
    assert!(
        matches!(
            &verdict,
            Verdict::Withheld(Decided { value: Withholding::Drop(reason), .. }) if reason == expected
        ),
        "{} should drop with {expected:?}, got {verdict:?}",
        clauses
            .first()
            .map_or("<no clauses>", |clause| clause.rule_name)
    );
}

pub(super) fn assert_clauses_allow(
    clauses: &[RuleClause],
    viewer: &ViewerFeatures,
    candidate: &HydratedTweetCandidate,
) {
    let verdict = clauses_verdict(clauses, viewer, candidate);
    assert!(
        matches!(
            verdict,
            Verdict::Shown {
                media: None,
                engagement: None,
            }
        ),
        "{} should allow, got {verdict:?}",
        clauses
            .first()
            .map_or("<no clauses>", |clause| clause.rule_name)
    );
}

pub(super) fn assert_drops(
    spec: &RuleClause,
    viewer: &ViewerFeatures,
    candidate: &HydratedTweetCandidate,
    expected: &FilteredReason,
) {
    assert_clauses_drop(from_ref(spec), viewer, candidate, expected);
}

pub(super) fn assert_allows(
    spec: &RuleClause,
    viewer: &ViewerFeatures,
    candidate: &HydratedTweetCandidate,
) {
    assert_clauses_allow(from_ref(spec), viewer, candidate);
}

pub(crate) fn viewer(id: u64) -> ViewerFeatures {
    ViewerFeatures {
        viewer: Viewer::LoggedIn(id),
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
    ViewerFeatures {
        allows_sensitive_media: true,
        ..viewer(VIEWER_ID)
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

    pub(crate) fn author_id(mut self, id: u64) -> Self {
        self.candidate.author_id = id;
        self
    }

    pub(crate) fn with_label(mut self, label: SafetyLabelType) -> Self {
        self.labels.insert(label);
        self
    }

    pub(crate) fn with_author_user_label(mut self, label: AuthorLabel) -> Self {
        self.candidate.author_features.user_labels.insert(label);
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

    pub(crate) fn followed(mut self) -> Self {
        self.candidate.relationship.viewer_follows_author = true;
        self
    }

    pub(crate) fn with_media(mut self) -> Self {
        self.candidate.tweet_features.media.has_media = true;
        self
    }

    pub(crate) fn retweet_of(mut self, source_tweet_id: u64) -> Self {
        self.candidate.tweet_features.core.source_tweet_id = Some(source_tweet_id);
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
