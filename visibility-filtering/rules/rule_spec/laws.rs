use super::*;
use crate::models::{ClientCapability, ViewerFeatures};
use crate::rules::fixtures::{candidate, viewer, VIEWER_ID};
use crate::rules::golden_corpus::pairs;
use crate::rules::SafetyLevel;
use std::iter::once;
use strum::VariantArray;

fn truths() -> Vec<Truth> {
    let a = Hydrators::of(Hydrator::Follows);
    let b = Hydrators::of(Hydrator::Blocks);
    let mut truths = vec![Truth::True, Truth::False];
    for default in [false, true] {
        for failed in [a, b, a.union(b)] {
            truths.push(Truth::Unknown { default, failed });
        }
    }
    truths
}

fn wired_clauses() -> Vec<&'static Clause> {
    let mut clauses: Vec<&'static Clause> = Vec::new();
    for &level in SafetyLevel::VARIANTS {
        for &clause in level.spec().rules {
            if !clauses.contains(&clause) {
                clauses.push(clause);
            }
        }
    }
    clauses
}

fn wired_leaves() -> Vec<Predicate> {
    let mut leaves = Vec::new();
    for clause in wired_clauses() {
        for condition in clause.when {
            let read = match &condition.0 {
                Condition::Is(leaf) | Condition::Not(leaf) => std::slice::from_ref(leaf),
                Condition::Any(set) => set,
            };
            for &leaf in read {
                if !leaves.contains(&leaf) {
                    leaves.push(leaf);
                }
            }
        }
    }
    leaves
}

#[test]
fn kleene_algebra() {
    let truths = truths();
    for &a in &truths {
        assert_eq!(a.and(Truth::True), a, "{a:?}");
        assert_eq!(a.or(Truth::False), a, "{a:?}");
        assert_eq!(a.and(Truth::False), Truth::False, "{a:?}");
        assert_eq!(a.or(Truth::True), Truth::True, "{a:?}");
        assert_eq!(!!a, a, "{a:?}");
        assert_eq!((!a).resolves_true(), !a.resolves_true(), "{a:?}");
        for &b in &truths {
            assert_eq!(a.and(b), b.and(a), "{a:?} {b:?}");
            assert_eq!(a.or(b), b.or(a), "{a:?} {b:?}");
            assert_eq!(!a.and(b), (!a).or(!b), "{a:?} {b:?}");
            assert_eq!(!a.or(b), (!a).and(!b), "{a:?} {b:?}");
            assert_eq!(
                a.and(b).resolves_true(),
                a.resolves_true() && b.resolves_true(),
                "{a:?} {b:?}"
            );
            assert_eq!(
                a.or(b).resolves_true(),
                a.resolves_true() || b.resolves_true(),
                "{a:?} {b:?}"
            );
            for &c in &truths {
                assert_eq!(a.and(b).and(c), a.and(b.and(c)), "{a:?} {b:?} {c:?}");
                assert_eq!(a.or(b).or(c), a.or(b.or(c)), "{a:?} {b:?} {c:?}");
            }
        }
    }
}

#[test]
fn a_failed_read_is_local() {
    let viewer = viewer(VIEWER_ID);
    let fetched = candidate().build();
    for leaf in wired_leaves() {
        let clean = leaf.truth(&test_context(&viewer, &fetched));
        for &hydrator in Hydrator::VARIANTS {
            let mut failed = fetched.clone();
            failed.failed = Hydrators::of(hydrator);
            let expected = if leaf.hydrators().contains(hydrator) {
                Truth::Unknown {
                    default: clean.resolves_true(),
                    failed: Hydrators::of(hydrator),
                }
            } else {
                clean
            };
            assert_eq!(
                leaf.truth(&test_context(&viewer, &failed)),
                expected,
                "{leaf:?} with {hydrator:?} failed"
            );
        }
    }
}

fn folded_without_break(clause: &Clause, context: &RuleContext<'_>) -> Truth {
    let mut truth = clause.audience.reaches(context);
    for condition in clause.when {
        truth = truth.and(condition.truth(context));
    }
    truth
}

fn holds(clause: &Clause, context: &RuleContext<'_>) -> bool {
    let author = context.is_author_viewer();
    let reached = match clause.audience {
        Audience::Everyone => true,
        Audience::NonAuthors => !author,
        Audience::NonFollowers => !author && !Predicate::ViewerFollowsAuthor.holds(context),
        Audience::OnlyAuthor => author,
    };
    reached
        && clause.when.iter().all(|condition| match condition.0 {
            Condition::Is(leaf) => leaf.holds(context),
            Condition::Not(leaf) => !leaf.holds(context),
            Condition::Any(set) => set.iter().any(|leaf| leaf.holds(context)),
        })
}

#[test]
fn the_clause_fold_is_faithful() {
    let clauses = wired_clauses();
    let failures: Vec<Hydrators> = once(Hydrators::empty())
        .chain(
            Hydrator::VARIANTS
                .iter()
                .map(|&hydrator| Hydrators::of(hydrator)),
        )
        .collect();
    for (viewer, fetched) in pairs() {
        for &failed in &failures {
            let mut candidate = fetched.clone();
            candidate.failed = failed;
            let context = test_context(&viewer, &candidate);
            for &clause in &clauses {
                if clause.hydrators().intersection(failed) != failed {
                    continue;
                }
                let truth = clause.applies(&context);
                assert_eq!(
                    truth,
                    folded_without_break(clause, &context),
                    "{} with {failed:?} failed",
                    clause.name()
                );
                assert_eq!(
                    truth.resolves_true(),
                    holds(clause, &context),
                    "{} with {failed:?} failed",
                    clause.name()
                );
            }
        }
    }
}

#[test]
fn the_age_gate_is_whole() {
    let unverified = not(viewer_age_verified());
    let age_gated = [
        request_country_in(CountryList::AgeVerification),
        request_country_in(CountryList::Tombstone),
    ];
    let mut gates = 0;
    for clause in wired_clauses() {
        let gated = clause
            .when
            .iter()
            .any(|condition| age_gated.contains(condition));
        assert_eq!(
            clause.when.contains(&unverified),
            gated,
            "{}: an age gate reads both not(viewer_age_verified()) and its country list",
            clause.name()
        );
        gates += usize::from(gated);
    }
    assert!(gates > 0, "no wired clause is age gated");
}

#[test]
fn every_client_capability_is_live() {
    let base = ClientCapability::default();
    let ClientCapability {
        verify_blur_support,
        modern_blur,
        stale_tweet_limits,
        community_viewer_removed_limits,
        gore_blur_ignores_settings,
        fosnr_rules,
        fosnr_fallback_drops,
    } = base;
    let blur_supports = once(None)
        .chain(VerifyBlurSupport::VARIANTS.iter().copied().map(Some))
        .filter(|&support| support != verify_blur_support)
        .map(|support| ClientCapability {
            verify_blur_support: support,
            ..base
        })
        .collect();
    let toggled: [(&str, Vec<ClientCapability>); 7] = [
        ("verify_blur_support", blur_supports),
        (
            "modern_blur",
            vec![ClientCapability {
                modern_blur: !modern_blur,
                ..base
            }],
        ),
        (
            "stale_tweet_limits",
            vec![ClientCapability {
                stale_tweet_limits: !stale_tweet_limits,
                ..base
            }],
        ),
        (
            "community_viewer_removed_limits",
            vec![ClientCapability {
                community_viewer_removed_limits: !community_viewer_removed_limits,
                ..base
            }],
        ),
        (
            "gore_blur_ignores_settings",
            vec![ClientCapability {
                gore_blur_ignores_settings: !gore_blur_ignores_settings,
                ..base
            }],
        ),
        (
            "fosnr_rules",
            vec![ClientCapability {
                fosnr_rules: !fosnr_rules,
                ..base
            }],
        ),
        (
            "fosnr_fallback_drops",
            vec![ClientCapability {
                fosnr_fallback_drops: !fosnr_fallback_drops,
                ..base
            }],
        ),
    ];
    let leaves = wired_leaves();
    let fetched = candidate().build();
    let holds = |client_capability: ClientCapability, leaf: Predicate| {
        let features = ViewerFeatures {
            client_capability,
            ..viewer(VIEWER_ID)
        };
        leaf.holds(&test_context(&features, &fetched))
    };
    for (field, capabilities) in toggled {
        let live = capabilities.iter().any(|&capability| {
            leaves
                .iter()
                .any(|&leaf| holds(capability, leaf) != holds(base, leaf))
        });
        assert!(
            live,
            "no wired leaf reads ClientCapability::{field}: wire a rule that reads it, or delete it"
        );
    }
}
