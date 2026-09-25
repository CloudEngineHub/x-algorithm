mod age_gating;
mod author_state;
mod baseline;
mod builders;
mod conversation_control;
mod exclusive_content;
mod interstitial;
mod oon_media;
mod oon_tweet_label;
mod oon_user_label;
mod relationship;
mod takedown;
mod tweet_label;
mod tweet_state;

use crate::models::{
    HydratedTweetCandidate, Verdict, ViewerAuthorRelationship, ViewerBlockedBy, ViewerFeatures,
};
use crate::rules::fixtures::{logged_out_viewer, viewer, VIEWER_ID};
use crate::rules::{RuleEngine, SafetyLevel};
use crate::treatment::proto_action;
use prost::Message;
use std::collections::BTreeSet;
use SafetyLevel::{
    FilterAll, ImmersiveExpandedRecommendations, TimelineHome, TimelineHomeHydration,
    TimelineHomeRecommendations,
};

enum Role {
    NonFollower,
    Author,
    Follower,
    LoggedOut,
    As(&'static str, ViewerFeatures),
}

struct Row {
    name: &'static str,
    post: HydratedTweetCandidate,
    expect: Vec<(SafetyLevel, Role, Verdict)>,
}

pub(super) struct CorpusCase {
    name: String,
    pub(super) level: SafetyLevel,
    pub(super) viewer: ViewerFeatures,
    pub(super) candidate: HydratedTweetCandidate,
    expected: Verdict,
}

impl Row {
    fn expand(self) -> impl Iterator<Item = CorpusCase> {
        let Row { name, post, expect } = self;
        expect.into_iter().map(move |(level, role, expected)| {
            let mut candidate = post.clone();
            let (role_name, viewer) = match role {
                Role::NonFollower => ("non_follower", viewer(VIEWER_ID)),
                Role::Author => ("author", viewer(candidate.author_id)),
                Role::Follower => {
                    candidate.relationship.viewer_follows_author = true;
                    ("follower", viewer(VIEWER_ID))
                }
                Role::LoggedOut => {
                    candidate.relationship = ViewerAuthorRelationship::default();
                    candidate.blocked_by = ViewerBlockedBy::default();
                    ("logged_out", logged_out_viewer())
                }
                Role::As(role_name, viewer) => (role_name, viewer),
            };
            CorpusCase {
                name: format!("{name}/{level:?}/{role_name}"),
                level,
                viewer,
                candidate,
                expected,
            }
        })
    }
}

fn deciders(verdict: &Verdict) -> Vec<&'static str> {
    match verdict {
        Verdict::Withheld(decided) => vec![decided.by],
        Verdict::Shown { media, engagement } => media
            .iter()
            .map(|blur| blur.by)
            .chain(engagement.iter().map(|limit| limit.by))
            .collect(),
    }
}

#[test]
fn golden_corpus_pins_policy_verdicts() {
    let rule_engine = RuleEngine::for_tests();
    let cases = corpus();
    let names: BTreeSet<&str> = cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names.len(), cases.len(), "duplicate corpus case name");
    let mut failures = Vec::new();
    for case in cases {
        let verdict = rule_engine.evaluate(case.level, &case.viewer, &case.candidate);
        if matches!(&case.expected, Verdict::Shown { media: Some(_), .. }) {
            let (action, reason) = proto_action(verdict.clone());
            assert_eq!(action.encode_to_vec(), [0x20, 0x01], "{}", case.name);
            assert_eq!(
                reason.unwrap().encode_to_vec(),
                [0x08, 0x01],
                "{}",
                case.name
            );
        }
        if verdict != case.expected {
            failures.push(format!(
                "{} [{:?}]:\n  expected {:?}\n  got      {:?}",
                case.name, case.level, case.expected, verdict,
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} corpus case(s) diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_wired_rule_decides_a_corpus_case() {
    let rule_engine = RuleEngine::for_tests();
    let wired: BTreeSet<&'static str> = [
        FilterAll,
        TimelineHome,
        TimelineHomeRecommendations,
        TimelineHomeHydration,
        ImmersiveExpandedRecommendations,
    ]
    .into_iter()
    .flat_map(|level| rule_engine.wired_rule_names(level))
    .collect();
    let deciders: BTreeSet<&'static str> = corpus()
        .iter()
        .flat_map(|c| deciders(&c.expected))
        .collect();
    let missing: Vec<&&'static str> = wired.difference(&deciders).collect();
    assert!(
        missing.is_empty(),
        "rules wired in RuleEngine but never the decider of any corpus case: {missing:?}"
    );
}

pub(super) fn corpus() -> Vec<CorpusCase> {
    rows().into_iter().flat_map(Row::expand).collect()
}

fn rows() -> Vec<Row> {
    [
        baseline::rows(),
        relationship::rows(),
        author_state::rows(),
        tweet_label::rows(),
        tweet_state::rows(),
        takedown::rows(),
        age_gating::rows(),
        exclusive_content::rows(),
        conversation_control::rows(),
        interstitial::rows(),
        oon_media::rows(),
        oon_tweet_label::rows(),
        oon_user_label::rows(),
    ]
    .into_iter()
    .flatten()
    .collect()
}
