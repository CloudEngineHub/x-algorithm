use super::conditions::{
    VIEWER_IS_NEITHER_ROOT_AUTHOR_NOR_INVITED,
    VIEWER_IS_NOT_AGE_VERIFIED_IN_AGE_VERIFICATION_COUNTRIES,
    VIEWER_IS_NOT_AGE_VERIFIED_IN_TOMBSTONE_COUNTRIES,
};
use super::engine::{Engine, Fold, PostEngine, UserEngine};
use super::features::{CompoundLeaf, ReplyControlLeaf, RequestLeaf};
use super::rule::Audience::{self, Everyone, NonAuthors, NonFollowers, OnlyAuthor};
use super::rule::{Rule, RuleCond};
use super::*;
use crate::hydration::inventory::Tier;
use crate::hydration::Hydrator;
use crate::models::{
    ClientCapability, PostCandidate, UserCandidate, VerifyBlurSupport, ViewerFeatures,
};
use crate::params::CountryLists;
use crate::rules::expansion::Expansion;
use crate::rules::fixtures::{candidate, viewer, VIEWER_ID};
use crate::rules::golden_corpus::{corpus, Subject};
use crate::rules::levels::PostRules;
use crate::rules::SafetyLevel;
use crate::rules::{placed_test_context, test_context};
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

fn wired<E: Engine>(list: fn(PostRules) -> &'static [&'static Rule<E>]) -> Vec<&'static Rule<E>> {
    let mut rules: Vec<&'static Rule<E>> = Vec::new();
    for &level in SafetyLevel::VARIANTS {
        for &rule in list(level.spec().rules.lists()) {
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
    rules
}

fn wired_user_rules() -> Vec<&'static Rule<UserEngine>> {
    wired(|rules| rules.user)
}

fn wired_rules() -> Vec<&'static Rule> {
    wired(|rules| rules.post)
}

fn wired_leaves() -> Vec<Predicate> {
    let mut read = Vec::new();
    for rule in wired_user_rules() {
        rule.audience.condition::<UserEngine>().leaves(&mut read);
        rule.when.leaves(&mut read);
    }
    for rule in wired_rules() {
        rule.audience.condition::<PostEngine>().leaves(&mut read);
        rule.when.leaves(&mut read);
    }
    let mut leaves = Vec::new();
    for leaf in read {
        if !leaves.contains(&leaf) {
            leaves.push(leaf);
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
            let expected = if leaf.requirements().contains(hydrator) {
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

const FAILS_THE_NOTIFICATION: &str =
    "FilterNotifications fails a notification whose verdict relied on a failed node";

const ACCEPTED_BEST_EFFORT_READS: &[(&str, Hydrator, &str)] = &[
    (
        "protected_author/drop",
        Hydrator::Follows,
        "the default drops the post for real followers",
    ),
    (
        "abusive_high_recall_user_label/drop/unspecified",
        Hydrator::Follows,
        "the default drops the post for real followers",
    ),
    (
        "do_not_amplify_user_label/drop/unspecified",
        Hydrator::Follows,
        "the default drops the post for real followers",
    ),
    (
        "quoted_viewer_mutes_author_not_following/drop",
        Hydrator::Follows,
        "the default drops the quote of a muted author the viewer really follows",
    ),
    (
        "protected_quoted_author/drop",
        Hydrator::OuterAuthorFollowsAuthor,
        "the default drops the quote of a protected author the quoting author really follows",
    ),
    (
        "fosnr_abuse_insults_non_follower/drop/undesirable",
        Hydrator::Follows,
        "the default drops the post for real followers",
    ),
    (
        "fosnr_abuse_insults_follower/soft_intervention/abuse",
        Hydrator::Follows,
        "the non-follower drop on the same label fires instead",
    ),
    (
        "author_blocks_viewer_exclusive_content/drop/unspecified",
        Hydrator::BlockedByAuthor,
        "only a subscriber the author blocked sees the exclusive post",
    ),
    (
        "viewer_mutes_and_does_not_follow_author/drop/viewer_mutes_author",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "no_confirmed_email/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "no_confirmed_phone/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "no_default_profile_image/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "no_new_users/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "no_not_followed_by/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "only_people_i_follow/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "not_graduated_user_label/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "agatha_spam_top_user_user_label/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "engagement_spammer_user_label/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "blink_worst_user_label/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
    (
        "downrank_spam_reply_user_label/drop/unspecified",
        Hydrator::Follows,
        FAILS_THE_NOTIFICATION,
    ),
];

#[test]
fn a_rule_reading_a_best_effort_node_limits_engagement_or_is_accepted() {
    let mut unaccepted = Vec::new();
    let mut accepted = Vec::new();
    let user = wired_user_rules().into_iter().map(|rule| {
        let limits = matches!(rule.action.spec(), ActionSpec::LimitedEngagement(_));
        (rule.name(), limits, rule.requirements(), Hydrators::empty())
    });
    let post = wired_rules().into_iter().map(|rule| {
        let limits = matches!(rule.action.spec(), ActionSpec::LimitedEngagement(_));
        let shown = rule.requirements().iter().filter(|node| {
            node.relation()
                .and_then(Expansion::of)
                .is_some_and(|expansion| {
                    let spec = expansion.spec();
                    spec.on_failure.tier() == Tier::BestEffort && spec.rules.contains(&rule)
                })
        });
        let shown = shown.fold(Hydrators::empty(), Hydrators::with);
        (rule.name(), limits, rule.requirements(), shown)
    });
    for (name, limits, requirements, shown) in user.chain(post) {
        for node in requirements.iter().filter(|node| node.is_best_effort()) {
            match ACCEPTED_BEST_EFFORT_READS
                .iter()
                .find(|&&(by, accepted, _)| by == name && accepted == node)
            {
                Some(&(by, node, _)) => accepted.push((by, node)),
                None if limits || shown.contains(node) => {}
                None => unaccepted.push((name.clone(), node)),
            }
        }
    }
    assert!(unaccepted.is_empty(), "{unaccepted:?}");
    for &(by, node, _) in ACCEPTED_BEST_EFFORT_READS {
        assert!(
            accepted.contains(&(by, node)),
            "{by} no longer reads {node:?}"
        );
    }
}

fn holds<E: Fold>(condition: RuleCond<E>, context: &RuleContext<'_, E::Candidate>) -> bool {
    match condition {
        RuleCond::Leaf(leaf) => truth_of(leaf, context).resolves_true(),
        RuleCond::All(children) => children.iter().all(|&child| holds(child, context)),
        RuleCond::Any(children) => children.iter().any(|&child| holds(child, context)),
        RuleCond::Not(child) => !holds(*child, context),
        RuleCond::Named(named) => holds(named.condition, context),
    }
}

fn faithful<E: Fold>(
    rules: &[&Rule<E>],
    context: &RuleContext<'_, E::Candidate>,
    failed: Hydrators,
) {
    for &rule in rules {
        if rule.requirements().intersection(failed) != failed {
            continue;
        }
        let (truth, ()) = observed::evaluate_rule(rule, context, &mut observed::NoTrace, None);
        let (total, _) =
            observed::evaluate_rule(rule, context, &mut observed::NodeTracer::default(), None);
        assert_eq!(truth, total, "{} with {failed:?} failed", rule.name());
        assert_eq!(
            truth.resolves_true(),
            holds(rule.audience.condition::<E>(), context) && holds(rule.when, context),
            "{} with {failed:?} failed",
            rule.name()
        );
    }
}

#[test]
fn the_rule_evaluator_is_faithful() {
    let (user_rules, rules) = (wired_user_rules(), wired_rules());
    let country_lists = CountryLists::starting_at_default();
    let failures: Vec<Hydrators> = once(Hydrators::empty())
        .chain(
            Hydrator::VARIANTS
                .iter()
                .map(|&hydrator| Hydrators::of(hydrator)),
        )
        .collect();
    for case in corpus() {
        let (user, post) = match case.subject {
            Subject::Post(post) => (UserCandidate::author_of(&post), Some(post)),
            Subject::User(user) => (user, None),
        };
        for &failed in &failures {
            let user = UserCandidate {
                failed,
                ..user.clone()
            };
            faithful(
                &user_rules,
                &RuleContext::new(&case.viewer, &user, case.placement, &country_lists),
                failed,
            );
            if let Some(post) = &post {
                let candidate = PostCandidate {
                    failed,
                    ..post.clone()
                };
                faithful(
                    &rules,
                    &placed_test_context(&case.viewer, &candidate, case.placement),
                    failed,
                );
            }
        }
    }
}

fn reads_one_definition<E: Fold>(
    rule: &Rule<E>,
    definitions: &[RuleCond<E>],
    leaves: &[Predicate],
) -> bool {
    fn walk<E: Engine>(
        condition: RuleCond<E>,
        definitions: &[RuleCond<E>],
        negated: bool,
        outside: &mut Vec<Predicate>,
        uses: &mut Vec<bool>,
    ) {
        match condition {
            condition if definitions.contains(&condition) => uses.push(negated),
            RuleCond::Leaf(leaf) => outside.push(E::lift(leaf)),
            RuleCond::All(children) | RuleCond::Any(children) => {
                for &child in children {
                    walk(child, definitions, negated, outside, uses);
                }
            }
            RuleCond::Not(child) => walk(*child, definitions, !negated, outside, uses),
            RuleCond::Named(named) => walk(named.condition, definitions, negated, outside, uses),
        }
    }
    let (mut outside, mut uses) = (Vec::new(), Vec::new());
    for root in [rule.audience.condition::<E>(), rule.when] {
        walk(root, definitions, false, &mut outside, &mut uses);
    }
    for leaf in leaves {
        assert!(
            !outside.contains(leaf),
            "{} reads {leaf:?} outside its named definition",
            rule.name()
        );
    }
    if !uses.is_empty() {
        assert_eq!(
            uses,
            [false],
            "{} must read one definition once, positively",
            rule.name()
        );
    }
    !uses.is_empty()
}

fn readers_through(definitions: &[RuleCond], leaves: &[Predicate]) -> usize {
    let user = wired_user_rules()
        .into_iter()
        .filter(|rule| reads_one_definition(rule, &[], leaves));
    let post = wired_rules()
        .into_iter()
        .filter(|rule| reads_one_definition(rule, definitions, leaves));
    user.count() + post.count()
}

#[test]
fn the_age_gate_is_whole() {
    let in_list = |list| Predicate::Request(RequestLeaf::RequestCountryIn(list));
    assert_ne!(
        readers_through(
            &[VIEWER_IS_NOT_AGE_VERIFIED_IN_AGE_VERIFICATION_COUNTRIES],
            &[in_list(CountryList::AgeVerification)]
        ),
        0
    );
    assert_ne!(
        readers_through(
            &[VIEWER_IS_NOT_AGE_VERIFIED_IN_TOMBSTONE_COUNTRIES],
            &[in_list(CountryList::Tombstone)]
        ),
        0
    );
    assert_ne!(
        readers_through(
            &[
                VIEWER_IS_NOT_AGE_VERIFIED_IN_AGE_VERIFICATION_COUNTRIES,
                VIEWER_IS_NOT_AGE_VERIFIED_IN_TOMBSTONE_COUNTRIES
            ],
            &[Predicate::Compound(CompoundLeaf::ViewerAgeVerified)]
        ),
        0
    );
}

#[test]
fn the_conversation_root_exemption_is_whole() {
    assert_ne!(
        readers_through(
            &[VIEWER_IS_NEITHER_ROOT_AUTHOR_NOR_INVITED],
            &[
                Predicate::ReplyControl(ReplyControlLeaf::ViewerIsConversationRootAuthor),
                Predicate::ReplyControl(ReplyControlLeaf::ViewerIsInvitedToConversation),
            ],
        ),
        0
    );
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
        soft_nsfw_new_user_drop,
    } = base;
    let blur_supports = once(None)
        .chain(VerifyBlurSupport::VARIANTS.iter().copied().map(Some))
        .filter(|&support| support != verify_blur_support)
        .map(|support| ClientCapability {
            verify_blur_support: support,
            ..base
        })
        .collect();
    let toggled: [(&str, Vec<ClientCapability>); 8] = [
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
        (
            "soft_nsfw_new_user_drop",
            vec![ClientCapability {
                soft_nsfw_new_user_drop: !soft_nsfw_new_user_drop,
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

#[test]
fn only_the_quote_pass_reads_quote_facts() {
    let mut readers = Vec::new();
    for &level in SafetyLevel::VARIANTS {
        let PostRules { user, post } = level.spec().rules.lists();
        let mut read = Vec::new();
        for rule in user {
            rule.when.leaves(&mut read);
        }
        for rule in post {
            rule.when.leaves(&mut read);
        }
        let reads_quote_facts = read.iter().any(|leaf| {
            matches!(
                leaf,
                Predicate::PairEdge(_) | Predicate::AuthorFacts(AuthorFactsLeaf::IsSelfQuote)
            )
        });
        if reads_quote_facts && level != SafetyLevel::QuotedTweetRules {
            readers.push(level);
        }
    }
    assert_eq!(
        readers,
        Vec::<SafetyLevel>::new(),
        "quote facts are hydrated only for the quote pass's inner-quoted authors"
    );
}

const POST_RULES_READING_NO_POST: &[(&str, &str)] = &[
    (
        "filter_all/drop/unspecified",
        "FilterAll's one rule drops every post, from either list",
    ),
    (
        "read_only_viewer/limited_engagement",
        "the user list would put it first among the engagement limits, so it would decide `by`",
    ),
];

#[test]
fn every_post_rule_reads_the_post() {
    let reads_the_post = |leaf: &Predicate| match leaf {
        Predicate::Request(_)
        | Predicate::ViewerAccount(_)
        | Predicate::ViewerLabel(_)
        | Predicate::NotificationFilter(_)
        | Predicate::AuthorFacts(_)
        | Predicate::AuthorSafety(_)
        | Predicate::AuthorLabels(_)
        | Predicate::AuthorProfile(_)
        | Predicate::AuthorEmail(_)
        | Predicate::AuthorPhone(_)
        | Predicate::Relationship(_)
        | Predicate::PairEdge(_) => false,
        Predicate::PostFacts(_)
        | Predicate::SourceTweetId(_)
        | Predicate::Tweet(_)
        | Predicate::TweetSafetyLabels(_)
        | Predicate::ReplyControl(_)
        | Predicate::PostModeration(_)
        | Predicate::ArticleStage(_)
        | Predicate::CommunityModerator(_)
        | Predicate::CommunityViewerRemoved(_)
        | Predicate::ReactedToPostSafetyLabels(_)
        | Predicate::ReactedToAuthorSafety(_)
        | Predicate::Edge(_)
        | Predicate::Compound(_) => true,
    };
    let reading_no_post: Vec<String> = wired_rules()
        .into_iter()
        .filter(|rule| {
            let mut read = Vec::new();
            rule.when.leaves(&mut read);
            let notice = matches!(
                rule.action.spec(),
                ActionSpec::SoftIntervention(_) | ActionSpec::Appealable(_)
            );
            !notice && !read.iter().any(reads_the_post)
        })
        .map(|rule| rule.name())
        .collect();
    assert_eq!(
        reading_no_post,
        POST_RULES_READING_NO_POST
            .iter()
            .map(|&(name, _)| name)
            .collect::<Vec<_>>(),
        "a post rule that reads no post is a `UserRule` in its levels' user lists, or a row of \
         POST_RULES_READING_NO_POST"
    );
}

#[test]
fn a_user_rule_that_ends_the_fold_never_shadows_a_more_severe_post_rule() {
    let ends_the_fold = |action: &ActionSpec<UserEngine>| match action {
        ActionSpec::Drop(_)
        | ActionSpec::Tombstone(_)
        | ActionSpec::SoftIntervention(_)
        | ActionSpec::Appealable(_) => true,
        ActionSpec::MediaRestriction(_)
        | ActionSpec::LimitedEngagement(_)
        | ActionSpec::LabelInterstitial(_) => false,
    };
    let disjoint = |user: Audience, post: Audience| match (user, post) {
        (OnlyAuthor, NonAuthors | NonFollowers) | (NonAuthors | NonFollowers, OnlyAuthor) => true,
        (Everyone, _)
        | (_, Everyone)
        | (OnlyAuthor, OnlyAuthor)
        | (NonAuthors | NonFollowers, NonAuthors | NonFollowers) => false,
    };
    let mut shadowing = Vec::new();
    for &level in SafetyLevel::VARIANTS {
        let PostRules { user, post } = level.spec().rules.lists();
        for &user_rule in user.iter().filter(|rule| ends_the_fold(rule.action.spec())) {
            let severity = user_rule.action.spec().severity();
            for &post_rule in post {
                if post_rule.action.spec().severity() > severity
                    && !disjoint(user_rule.audience, post_rule.audience)
                {
                    shadowing.push(format!(
                        "{level:?}: user {} shadows post {}",
                        user_rule.name(),
                        post_rule.name()
                    ));
                }
            }
        }
    }
    assert_eq!(
        shadowing,
        Vec::<String>::new(),
        "the user list folds before every post rule, so a user rule that ends the fold needs an \
         audience disjoint from every more severe post rule"
    );
}
