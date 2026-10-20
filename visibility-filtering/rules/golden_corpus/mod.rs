mod age_gating;
mod age_verification;
mod article;
mod author_state;
mod baseline;
mod builders;
mod community;
mod conversation_control;
mod exclusive_content;
mod interstitial;
mod legacy_interstitial;
mod local_regulations;
mod local_tweet;
mod oon_media;
mod oon_tweet_label;
mod oon_user_label;
mod quoted_tweet_rules;
mod relationship;
mod takedown;
mod trusted_friends;
mod tweet_label;
mod tweet_state;

use crate::hydration::{Hydrator, Hydrators};
use crate::models::{
    Decided, Evaluation, Notice, Placement, PostCandidate, QuoteFacts, TweetId, UserCandidate,
    Verdict, ViewerFeatures, Withholding,
};
use crate::rules::engine::Policy;
use crate::rules::fixtures::{logged_out_viewer, viewer, VIEWER_ID};
use crate::rules::levels::PostRules;
use crate::rules::rule_spec::engine::{Engine, UserEngine};
use crate::rules::rule_spec::rule::{NamedCond, Rule, RuleCond};
use crate::rules::{placed_test_context, RuleEngine, SafetyLevel};
use crate::treatment::proto_action;
use prost::Message;
use std::collections::BTreeSet;
use std::iter::once;
use strum::VariantArray;
use SafetyLevel::{
    FilterAll, ImmersiveExpandedRecommendations, QuotedTweetRules, TimelineHome,
    TimelineHomeHydration, TimelineHomeRecommendations,
};

enum Role {
    NonFollower,
    Author,
    Follower,
    LoggedOut,
    As(&'static str, ViewerFeatures),
    InnerQuoted(Box<Role>),
    RetweetSource(Box<Role>),
}

struct Row {
    name: &'static str,
    post: PostCandidate,
    expect: Vec<(SafetyLevel, Role, Verdict)>,
}

struct UserRow {
    name: &'static str,
    role: &'static str,
    viewer: ViewerFeatures,
    user: UserCandidate,
    quote: QuoteFacts,
    expected: Verdict,
}

struct CorpusCase {
    name: String,
    level: SafetyLevel,
    viewer: ViewerFeatures,
    candidate: PostCandidate,
    placement: Placement,
    expected: Verdict,
}

const OUTER_TWEET_ID: TweetId = TweetId(2);

impl Role {
    fn view(&self, candidate: &mut PostCandidate) -> (String, ViewerFeatures, Placement) {
        let standalone = |name: &str, viewer| (name.to_owned(), viewer, Placement::Standalone);
        match self {
            Role::NonFollower => standalone("non_follower", viewer(VIEWER_ID)),
            Role::Author => standalone("author", viewer(candidate.author.id)),
            Role::Follower => {
                candidate.viewer_relationship.follows = true;
                standalone("follower", viewer(VIEWER_ID))
            }
            Role::LoggedOut => {
                candidate.viewer_relationship = Default::default();
                candidate.post.edges = Hydrators::empty();
                standalone("logged_out", logged_out_viewer())
            }
            Role::As(role_name, viewer) => standalone(role_name, viewer.clone()),
            Role::InnerQuoted(role) => {
                let (role_name, viewer, _) = role.view(candidate);
                (
                    format!("inner_quoted_{role_name}"),
                    viewer,
                    Placement::InnerQuoted {
                        outer: OUTER_TWEET_ID,
                    },
                )
            }
            Role::RetweetSource(role) => {
                let (role_name, viewer, _) = role.view(candidate);
                (
                    format!("retweet_source_{role_name}"),
                    viewer,
                    Placement::RetweetSource,
                )
            }
        }
    }
}

impl Row {
    fn expand(self) -> impl Iterator<Item = CorpusCase> {
        let Row { name, post, expect } = self;
        expect.into_iter().map(move |(level, role, expected)| {
            let mut candidate = post.clone();
            let (role_name, viewer, placement) = role.view(&mut candidate);
            CorpusCase {
                name: format!("{name}/{level:?}/{role_name}"),
                level,
                viewer,
                candidate,
                placement,
                expected,
            }
        })
    }
}

fn deciders(verdict: &Verdict) -> Vec<&'static str> {
    match verdict {
        Verdict::Withheld(decided) => vec![decided.by],
        Verdict::Shown {
            notice,
            media,
            engagement,
        } => notice
            .iter()
            .map(|notice| notice.by)
            .chain(media.iter().map(|blur| blur.by))
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
        let verdict = rule_engine
            .evaluate(case.level, &case.viewer, &case.candidate, case.placement)
            .into_verdict();
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
fn the_user_table_pins_quoted_tweet_rules_verdicts() {
    let rule_engine = RuleEngine::for_tests();
    let rows = quoted_tweet_rules::rows();
    let names: BTreeSet<(&str, &str)> = rows.iter().map(|row| (row.name, row.role)).collect();
    assert_eq!(names.len(), rows.len(), "duplicate user table row");
    let failures: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            let verdict = rule_engine
                .evaluate_user(
                    QuotedTweetRules,
                    &row.viewer,
                    &row.user,
                    Placement::InnerQuoted {
                        outer: OUTER_TWEET_ID,
                    },
                    row.quote,
                )
                .into_verdict();
            (verdict != row.expected).then(|| {
                format!(
                    "{}/{}:\n  expected {:?}\n  got      {:?}",
                    row.name, row.role, row.expected, verdict
                )
            })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} user table row(s) diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
#[ignore]
fn bench_corpus_evaluation() {
    use std::hint::black_box;
    use std::time::Instant;

    const ROUNDS: u32 = 2_000;
    let rule_engine = RuleEngine::for_tests();
    let cases = corpus();
    let home_hydration: Vec<&CorpusCase> = cases
        .iter()
        .filter(|case| case.level == TimelineHomeHydration)
        .collect();
    for (label, cases) in [
        ("all", cases.iter().collect::<Vec<_>>()),
        ("home_hydration", home_hydration),
    ] {
        let start = Instant::now();
        for _ in 0..ROUNDS {
            for case in &cases {
                black_box(rule_engine.evaluate(
                    case.level,
                    black_box(&case.viewer),
                    black_box(&case.candidate),
                    case.placement,
                ));
            }
        }
        let evaluations = u128::from(ROUNDS) * cases.len() as u128;
        println!(
            "BENCH {label}: {} cases, {} ns/evaluation",
            cases.len(),
            start.elapsed().as_nanos() / evaluations.max(1)
        );
    }
}

#[test]
fn every_node_failing_changes_no_corpus_verdict_and_only_a_must_have_one_makes_it_partial() {
    let rule_engine = RuleEngine::for_tests();
    let best_effort = Hydrators::all().difference(Hydrators::all().must_have());
    for mut case in corpus() {
        for (failed, is_partial) in [(best_effort, false), (Hydrators::all(), true)] {
            case.candidate.failed = failed;
            let evaluation =
                rule_engine.evaluate(case.level, &case.viewer, &case.candidate, case.placement);
            assert_eq!(
                matches!(evaluation, Evaluation::Partial { .. }),
                is_partial,
                "{} with {failed:?} failed",
                case.name
            );
            assert_eq!(evaluation.into_verdict(), case.expected, "{}", case.name);
        }
    }
}

fn lifted(condition: RuleCond<UserEngine>) -> RuleCond {
    let children = |children: &[RuleCond<UserEngine>]| -> &'static [RuleCond] {
        children
            .iter()
            .map(|&child| lifted(child))
            .collect::<Vec<_>>()
            .leak()
    };
    match condition {
        RuleCond::Leaf(leaf) => RuleCond::Leaf(UserEngine::lift(leaf)),
        RuleCond::All(all) => RuleCond::All(children(all)),
        RuleCond::Any(any) => RuleCond::Any(children(any)),
        RuleCond::Not(child) => RuleCond::Not(Box::leak(Box::new(lifted(*child)))),
        RuleCond::Named(named) => RuleCond::Named(Box::leak(Box::new(NamedCond {
            name: named.name,
            condition: lifted(named.condition),
        }))),
    }
}

fn one_fold(PostRules { user, post }: PostRules) -> Policy {
    let user = user.iter().map(|rule| Rule {
        id: rule.id,
        audience: rule.audience,
        when: lifted(rule.when),
        action: rule.action.lift(),
    });
    let rules = user.chain(post.iter().map(|&rule| rule.clone()));
    Policy::new(rules.map(|rule| (&*rule.name().leak(), rule)).collect())
}

fn outcome(evaluation: &Evaluation) -> (&'static str, Vec<&'static str>) {
    let completeness = match evaluation {
        Evaluation::Complete { .. } => "Complete",
        Evaluation::Partial { .. } => "Partial",
        Evaluation::NotFound(_) => "NotFound",
        Evaluation::Failed(_) => "Failed",
    };
    let kinds = match evaluation.verdict() {
        Verdict::Withheld(Decided {
            value: Withholding::Drop(_),
            ..
        }) => vec!["Drop"],
        Verdict::Withheld(Decided {
            value: Withholding::Tombstone(_),
            ..
        }) => vec!["Tombstone"],
        Verdict::Shown {
            notice,
            media,
            engagement,
        } => {
            let kinds: Vec<&'static str> = notice
                .iter()
                .map(|notice| match notice.value {
                    Notice::Appealable { .. } => "Appealable",
                    Notice::SoftIntervention(_) => "SoftIntervention",
                })
                .chain(media.iter().map(|_| "MediaRestriction"))
                .chain(engagement.iter().map(|_| "LimitedEngagement"))
                .collect();
            if kinds.is_empty() {
                vec!["Allow"]
            } else {
                kinds
            }
        }
    };
    (completeness, kinds)
}

#[test]
fn the_user_gate_keeps_the_outcome_of_one_severity_sorted_fold() {
    let rule_engine = RuleEngine::for_tests();
    let cases = corpus();
    let mut diverged = Vec::new();
    for &level in SafetyLevel::VARIANTS {
        let rules = level.spec().rules.lists();
        let reference = one_fold(rules);
        let used = rules
            .user
            .iter()
            .map(|rule| rule.requirements())
            .chain(rules.post.iter().map(|rule| rule.requirements()))
            .fold(Hydrators::empty(), Hydrators::union);
        let failures: Vec<Hydrators> = once(Hydrators::empty())
            .chain(used.iter().map(Hydrators::of))
            .chain(once(used))
            .collect();
        for case in &cases {
            for &failed in &failures {
                let candidate = PostCandidate {
                    failed,
                    ..case.candidate.clone()
                };
                let gate = rule_engine.evaluate(level, &case.viewer, &candidate, case.placement);
                let fold = reference.evaluate(&placed_test_context(
                    &case.viewer,
                    &candidate,
                    case.placement,
                ));
                if outcome(&gate) != outcome(&fold) {
                    diverged.push(format!(
                        "{level:?} {} with {:?} failed:\n  gate {gate:?}\n  fold {fold:?}",
                        case.name,
                        failed.iter().collect::<Vec<Hydrator>>()
                    ));
                }
            }
        }
    }
    assert!(
        diverged.is_empty(),
        "{} evaluation(s) left the outcome of one severity-sorted fold:\n{}",
        diverged.len(),
        diverged.join("\n")
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
        QuotedTweetRules,
    ]
    .into_iter()
    .flat_map(|level| rule_engine.wired_rule_names(level))
    .collect();
    let deciders: BTreeSet<&'static str> = corpus()
        .iter()
        .flat_map(|c| deciders(&c.expected))
        .chain(
            quoted_tweet_rules::rows()
                .iter()
                .flat_map(|row| deciders(&row.expected)),
        )
        .collect();
    let missing: Vec<&&'static str> = wired.difference(&deciders).collect();
    assert!(
        missing.is_empty(),
        "rules wired in RuleEngine but never the decider of any corpus case: {missing:?}"
    );
}

fn corpus() -> Vec<CorpusCase> {
    rows().into_iter().flat_map(Row::expand).collect()
}

pub(super) fn cases() -> Vec<(SafetyLevel, ViewerFeatures, PostCandidate)> {
    corpus()
        .into_iter()
        .map(|case| (case.level, case.viewer, case.candidate))
        .collect()
}

pub(super) fn pairs() -> Vec<(ViewerFeatures, PostCandidate)> {
    cases()
        .into_iter()
        .map(|(_, viewer, candidate)| (viewer, candidate))
        .collect()
}

fn rows() -> Vec<Row> {
    [
        baseline::rows(),
        relationship::rows(),
        author_state::rows(),
        tweet_label::rows(),
        tweet_state::rows(),
        community::rows(),
        takedown::rows(),
        article::rows(),
        age_gating::rows(),
        age_verification::rows(),
        exclusive_content::rows(),
        trusted_friends::rows(),
        conversation_control::rows(),
        interstitial::rows(),
        legacy_interstitial::rows(),
        local_regulations::rows(),
        local_tweet::rows(),
        oon_media::rows(),
        oon_tweet_label::rows(),
        oon_user_label::rows(),
    ]
    .into_iter()
    .flatten()
    .collect()
}
