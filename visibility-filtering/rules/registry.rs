use crate::hydration::{HydrationPlan, Hydrators};
use crate::models::{
    Decided, Evaluation, FosnrReason, LimitedEngagement, Notice, PostCandidate, SafetyLabelMap,
    SafetyLabelType, Verdict, ViewerFeatures, Withholding,
};
use crate::params::{CountryList, CountryLists};
use crate::rules::rule_spec::{
    ActionSpec, Clause, ClauseAction, Cond, FosnrViolation, RuleId, Truth,
    viewer_account_or_request_country_in, viewer_blocks_author, viewer_follows_author,
    viewer_is_blocked_by_author, viewer_mutes_author,
};
use crate::rules::{RuleContext, SafetyLevel};
use std::cmp::Reverse;
use std::sync::Arc;
use strum::VariantArray;
use xai_x_thrift::action::AppealablePolicy;

pub(super) struct Policy {
    clauses: Vec<(&'static str, Clause)>,
    hydrators: Hydrators,
}

impl Policy {
    fn new(mut clauses: Vec<(&'static str, Clause)>) -> Self {
        clauses.sort_by_key(|(_, clause)| Reverse(clause.action.spec().severity()));
        let mut runs: Vec<&'static str> = Vec::new();
        let mut run: Vec<&Clause> = Vec::new();
        for &(name, ref clause) in &clauses {
            if runs.last() != Some(&name) {
                assert!(!runs.contains(&name), "{name} is wired twice");
                runs.push(name);
                run.clear();
            }
            assert!(!run.contains(&clause), "{name} is wired twice");
            run.push(clause);
        }
        let hydrators = clauses
            .iter()
            .fold(Hydrators::empty(), |hydrators, (_, clause)| {
                hydrators.union(clause.hydrators())
            });
        Self { clauses, hydrators }
    }

    pub(super) fn evaluate(&self, context: &RuleContext<'_>) -> Evaluation {
        let (verdict, fail_open_defaults) = self.decide(context);
        if context.failed().is_empty() {
            debug_assert!(
                fail_open_defaults.is_empty(),
                "a verdict over no failed node relies on no default"
            );
            Evaluation::Complete { verdict }
        } else {
            Evaluation::Partial {
                verdict,
                fail_open_defaults,
            }
        }
    }

    fn decide(&self, context: &RuleContext<'_>) -> (Verdict, Hydrators) {
        let mut media = None;
        let mut engagement = None;
        let mut withholding_defaults = Hydrators::empty();
        let mut slot_defaults = Hydrators::empty();

        for &(by, ref rule) in &self.clauses {
            let truth = rule.applies(context);
            let unknown_reads = match truth {
                Truth::Unknown { failed, .. } => failed,
                Truth::True | Truth::False => Hydrators::empty(),
            };
            match rule.action.spec() {
                ActionSpec::Appealable(violations) => {
                    withholding_defaults = withholding_defaults.union(unknown_reads);
                    if truth.resolves_true()
                        && let Some(notice) = appeal(violations, context.tweet_safety_labels())
                    {
                        return (
                            Verdict::Shown {
                                notice: Some(Decided { value: notice, by }),
                                media: None,
                                engagement: None,
                            },
                            withholding_defaults,
                        );
                    }
                }
                ActionSpec::Drop(reason) => {
                    withholding_defaults = withholding_defaults.union(unknown_reads);
                    if truth.resolves_true() {
                        return (
                            Verdict::Withheld(Decided {
                                value: Withholding::Drop(reason.clone()),
                                by,
                            }),
                            withholding_defaults,
                        );
                    }
                }
                ActionSpec::Tombstone(reason) => {
                    withholding_defaults = withholding_defaults.union(unknown_reads);
                    if truth.resolves_true() {
                        return (
                            Verdict::Withheld(Decided {
                                value: Withholding::Tombstone(*reason),
                                by,
                            }),
                            withholding_defaults,
                        );
                    }
                }
                ActionSpec::MediaRestriction(value) if media.is_none() => {
                    slot_defaults = slot_defaults.union(unknown_reads);
                    if truth.resolves_true() {
                        media = Some(Decided {
                            value: value.clone(),
                            by,
                        });
                    }
                }
                ActionSpec::LimitedEngagement(reason) => {
                    if engagement.is_none() {
                        slot_defaults = slot_defaults.union(unknown_reads);
                    }
                    if truth.resolves_true() {
                        match &mut engagement {
                            None => {
                                engagement = Some(Decided {
                                    value: LimitedEngagement::new(*reason),
                                    by,
                                });
                            }
                            Some(limit) => limit.value.add(*reason),
                        }
                    }
                }
                ActionSpec::MediaRestriction(_) => {}
                ActionSpec::SoftIntervention {
                    label,
                    policy,
                    level,
                } if media.is_none() => {
                    slot_defaults = slot_defaults.union(unknown_reads);
                    if truth.resolves_true() {
                        let labels = context.tweet_safety_labels();
                        return (
                            Verdict::Shown {
                                notice: Some(Decided {
                                    value: Notice::SoftIntervention(fosnr_reason(
                                        *label, *policy, *level, labels,
                                    )),
                                    by,
                                }),
                                media: None,
                                engagement: None,
                            },
                            withholding_defaults.union(slot_defaults),
                        );
                    }
                }
                ActionSpec::SoftIntervention { .. } => {}
            }
        }

        (
            Verdict::Shown {
                notice: None,
                media,
                engagement,
            },
            withholding_defaults.union(slot_defaults),
        )
    }

    fn rule_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        let mut previous: Option<&'static str> = None;
        self.clauses.iter().filter_map(move |&(name, _)| {
            let repeat = previous == Some(name);
            previous = Some(name);
            (!repeat).then_some(name)
        })
    }

    fn len(&self) -> usize {
        self.rule_names().count()
    }
}

fn appeal(violations: &[FosnrViolation], labels: &SafetyLabelMap) -> Option<Notice> {
    let violation = violations
        .iter()
        .find(|violation| labels.has_label(violation.label))?;
    Some(Notice::Appealable {
        reason: fosnr_reason(violation.label, violation.policy, violation.level, labels),
        limited_actions: violation.limited_actions,
    })
}

fn fosnr_reason(
    label: SafetyLabelType,
    policy: AppealablePolicy,
    level: i8,
    labels: &SafetyLabelMap,
) -> FosnrReason {
    FosnrReason {
        policy,
        level,
        proactive: !labels.is_by_agent(label),
        appeal_submitted: labels.has_label(SafetyLabelType::FOSNR_APPEAL_SUBMITTED),
    }
}

struct Level {
    policy: Policy,
    plan: HydrationPlan,
}

pub struct RuleEngine {
    country_lists: Arc<CountryLists>,
    levels: Vec<Level>,
}

#[expect(
    clippy::panic,
    reason = "invalid authored rules fail startup, before the service takes traffic"
)]
fn interned_name(
    names: &mut Vec<(RuleId, ClauseAction, &'static str)>,
    clause: &Clause,
) -> &'static str {
    if let Some(&(_, _, name)) = names
        .iter()
        .find(|(id, action, _)| *id == clause.id && *action == clause.action)
    {
        return name;
    }
    let name: &'static str = clause.name().leak();
    if let Some((id, action, _)) = names.iter().find(|&&(_, _, other)| other == name) {
        panic!(
            "{id:?} {action:?} and {:?} {:?} derive {name}",
            clause.id, clause.action
        );
    }
    names.push((clause.id, clause.action.clone(), name));
    name
}

impl RuleEngine {
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::with_country_lists(Arc::new(CountryLists::starting_at_default()))
    }

                pub fn with_country_lists(country_lists: Arc<CountryLists>) -> Self {
        let mut names = Vec::new();
        let levels = SafetyLevel::VARIANTS
            .iter()
            .map(|&level| {
                let clauses = level
                    .spec()
                    .rules
                    .iter()
                    .map(|&clause| (interned_name(&mut names, clause), clause.clone()))
                    .collect();
                let policy = Policy::new(clauses);
                let plan = HydrationPlan::new(level, policy.hydrators);
                Level { policy, plan }
            })
            .collect();
        Self {
            country_lists,
            levels,
        }
    }

    pub fn evaluate(
        &self,
        level: SafetyLevel,
        viewer: &ViewerFeatures,
        candidate: &PostCandidate,
    ) -> Evaluation {
        let policy = self.policy(level);
        let context = RuleContext::new(viewer, candidate, &self.country_lists);
        #[cfg(test)]
        let context = context.hydrated_by(policy.hydrators);
        policy.evaluate(&context)
    }

    fn level(&self, level: SafetyLevel) -> &Level {
        #[expect(
            clippy::indexing_slicing,
            reason = "`levels` maps `SafetyLevel::VARIANTS`, which is in declaration order"
        )]
        let entry = &self.levels[level as usize];
        debug_assert_eq!(entry.plan.level(), level);
        entry
    }

    fn policy(&self, level: SafetyLevel) -> &Policy {
        &self.level(level).policy
    }

    pub(crate) fn plan(&self, level: SafetyLevel) -> &HydrationPlan {
        &self.level(level).plan
    }

    #[cfg(test)]
    pub(crate) fn wired_rule_names(&self, level: SafetyLevel) -> Vec<&'static str> {
        self.policy(level).rule_names().collect()
    }

    pub fn rule_counts(&self) -> (usize, usize) {
        (
            self.policy(SafetyLevel::TimelineHome).len(),
            self.policy(SafetyLevel::TimelineHomeRecommendations).len(),
        )
    }

    pub(crate) fn truth(
        &self,
        fact: ExplainedFact,
        viewer: &ViewerFeatures,
        candidate: &PostCandidate,
    ) -> Truth {
        fact.leaf()
            .truth(&RuleContext::new(viewer, candidate, &self.country_lists))
    }

    pub(crate) fn in_nsfw_gating_country(&self, viewer: &ViewerFeatures) -> Truth {
        viewer_account_or_request_country_in(CountryList::NsfwGating).truth(&RuleContext::new(
            viewer,
            &PostCandidate::default(),
            &self.country_lists,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExplainedFact {
    Follows,
    Blocks,
    Mutes,
    IsBlockedBy,
}

impl ExplainedFact {
    const fn leaf(self) -> Cond {
        match self {
            Self::Follows => viewer_follows_author(),
            Self::Blocks => viewer_blocks_author(),
            Self::Mutes => viewer_mutes_author(),
            Self::IsBlockedBy => viewer_is_blocked_by_author(),
        }
    }

    pub(crate) const fn reads(self) -> Hydrators {
        self.leaf().hydrators()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hydration::Hydrator;
    use crate::models::{
        AuthorFeatures, ClientCapability, DropReason, NsfwViewerDropReason, SafetyLabelType,
        VerifyBlurSupport, ViewerAge, ViewerProfile,
    };
    use crate::rules::fixtures::{
        CandidateBuilder, VIEWER_ID, candidate, viewer, viewer_with_profile,
    };
    use crate::rules::rule_spec::Spelled;
    use crate::rules::{levels, test_context, tweet_rules};
    use std::fmt::Write;

    #[test]
    fn refreshed_config_country_reaches_the_wired_rule() {
        let country_lists = Arc::new(CountryLists::starting_at_default());
        let rule_engine = RuleEngine::with_country_lists(Arc::clone(&country_lists));
        let candidate = candidate()
            .with_label(crate::models::SafetyLabelType::NSFW_HIGH_PRECISION)
            .with_media()
            .build();
        let viewer = ViewerFeatures {
            country_code: Some("us".into()),
            ..viewer_with_profile(ViewerProfile {
                viewer_age: ViewerAge::NotStated,
                ..ViewerProfile::default()
            })
        };

        let verdict = rule_engine
            .evaluate(SafetyLevel::TimelineHome, &viewer, &candidate)
            .into_verdict();
        assert!(!matches!(verdict, Verdict::Withheld(_)));

        country_lists.refresh(
            &xai_feature_switches::FeatureSwitches::load_string(
                r#"
country_specific_nsfw_content_gating:
  parameters:
    country_specific_nsfw_content_gating_countries:
      type: array
      default:
      - "us"
"#,
            )
            .unwrap(),
        );
        let verdict = rule_engine
            .evaluate(SafetyLevel::TimelineHome, &viewer, &candidate)
            .into_verdict();
        assert!(matches!(
            verdict,
            Verdict::Withheld(Decided {
                value: Withholding::Drop(DropReason::NsfwViewer(
                    NsfwViewerDropReason::HasNoStatedAge
                )),
                by: "sensitive_viewer_no_stated_age/drop",
            })
        ));
    }

    #[test]
    fn a_failed_article_lookup_is_a_partial_drop() {
        use crate::models::TweetFeatures;
        use crate::rules::fixtures::dropped;
        use std::num::NonZeroU64;
        use xai_visibility_filtering::models::FilteredReason;

        let mut candidate = candidate()
            .with_tweet_features(TweetFeatures {
                article_id: NonZeroU64::new(1),
                ..TweetFeatures::default()
            })
            .build();
        candidate.failed = Hydrators::of(Hydrator::ArticleLifecycle);
        assert_eq!(
            RuleEngine::for_tests().evaluate(
                SafetyLevel::TimelineHomeHydration,
                &viewer(VIEWER_ID),
                &candidate,
            ),
            Evaluation::Partial {
                verdict: dropped(
                    FilteredReason::UnspecifiedReason,
                    "article_tweet_content/drop/unspecified",
                ),
                fail_open_defaults: Hydrators::of(Hydrator::ArticleLifecycle),
            }
        );
    }

    #[test]
    fn age_verification_tombstones_read_their_family_country_list() {
        let country_lists = Arc::new(CountryLists::starting_at_default());
        country_lists.refresh(
            &xai_feature_switches::FeatureSwitches::load_string(
                r#"
country_specific_nsfw_content_gating:
  parameters:
    country_specific_nsfw_content_gating_tombstone_countries:
      type: array
      default:
      - "us"
"#,
            )
            .unwrap(),
        );
        let rule_engine = RuleEngine::with_country_lists(country_lists);
        let viewer = ViewerFeatures {
            country_code: Some("us".into()),
            client_capability: ClientCapability {
                verify_blur_support: Some(VerifyBlurSupport::IosNeedsUpdate),
                modern_blur: true,
                stale_tweet_limits: true,
                community_viewer_removed_limits: true,
                gore_blur_ignores_settings: true,
                fosnr_rules: true,
                fosnr_fallback_drops: false,
            },
            ..viewer(VIEWER_ID)
        };
        let nsfw_admin = AuthorFeatures {
            is_nsfw_admin: true,
            ..AuthorFeatures::default()
        };
        let withheld_by = |candidate: CandidateBuilder| match rule_engine
            .evaluate(
                SafetyLevel::TimelineHomeHydration,
                &viewer,
                &candidate.with_media().build(),
            )
            .into_verdict()
        {
            Verdict::Withheld(Decided { by, .. }) => Some(by),
            Verdict::Shown { .. } => None,
        };
        let labeled = |label| candidate().with_label(label);
        assert_eq!(
            withheld_by(labeled(SafetyLabelType::NSFW_HIGH_PRECISION)),
            Some("nsfw_high_precision/tombstone/update_app_ios")
        );
        assert_eq!(
            withheld_by(labeled(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION)),
            Some("gore_and_violence_high_precision/tombstone/update_app_ios")
        );
        assert_eq!(
            withheld_by(candidate().with_author_features(nsfw_admin)),
            None
        );
        assert_eq!(
            withheld_by(labeled(SafetyLabelType::NSFW_REPORTED_HEURISTICS)),
            None
        );
        assert_eq!(withheld_by(labeled(SafetyLabelType::NSFW_CARD_IMAGE)), None);
    }

    const PINNED_POLICIES: [(SafetyLevel, &str); 5] = [
        (
            SafetyLevel::FilterAll,
            include_str!("../../tests/fixtures/policy/filter_all.txt"),
        ),
        (
            SafetyLevel::TimelineHome,
            include_str!("../../tests/fixtures/policy/timeline_home.txt"),
        ),
        (
            SafetyLevel::TimelineHomeRecommendations,
            include_str!("../../tests/fixtures/policy/timeline_home_recommendations.txt"),
        ),
        (
            SafetyLevel::TimelineHomeHydration,
            include_str!("../../tests/fixtures/policy/timeline_home_hydration.txt"),
        ),
        (
            SafetyLevel::ImmersiveExpandedRecommendations,
            include_str!("../../tests/fixtures/policy/immersive_expanded_recommendations.txt"),
        ),
    ];

    fn band(action: &ActionSpec) -> &'static str {
        match action {
            ActionSpec::Appealable(_) => "appealable, notice slot",
            ActionSpec::Drop(_) => "drop, withholding",
            ActionSpec::Tombstone(_) => "tombstone, withholding",
            ActionSpec::MediaRestriction(_) => "media restriction, media slot",
            ActionSpec::SoftIntervention { .. } => "soft intervention, notice slot",
            ActionSpec::LimitedEngagement(_) => "limited engagement, engagement slot",
        }
    }

    fn action_column(action: &ActionSpec) -> String {
        match action {
            ActionSpec::Drop(reason) => format!("{reason:?}"),
            ActionSpec::Tombstone(reason) => format!("{reason:?}"),
            ActionSpec::MediaRestriction(restriction) => format!("{restriction:?}"),
            ActionSpec::SoftIntervention {
                label,
                policy,
                level,
            } => format!("{} {policy:?} level {level}", label.spelled()),
            ActionSpec::LimitedEngagement(reason) => format!("{reason:?}"),
            ActionSpec::Appealable(violations) => violations
                .iter()
                .map(
                    |&FosnrViolation {
                         label,
                         policy,
                         level,
                         limited_actions,
                     }| {
                        format!(
                            "{} {policy:?} level {level} limits {limited_actions:?}",
                            label.spelled()
                        )
                    },
                )
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    fn policy_table(policy: &Policy) -> String {
        let mut table = String::new();
        let mut severity = None;
        for (name, clause) in &policy.clauses {
            let action = clause.action.spec();
            if severity != Some(action.severity()) {
                severity = Some(action.severity());
                writeln!(table, "## {}", band(action)).unwrap();
            }
            let when: Vec<String> = clause
                .when
                .iter()
                .map(|condition| condition.spelled())
                .collect();
            let when = if when.is_empty() {
                "always".to_owned()
            } else {
                when.join(" & ")
            };
            writeln!(
                table,
                "{name} | {:?} | {when} | {}",
                clause.audience,
                action_column(action)
            )
            .unwrap();
        }
        table
    }

    #[test]
    fn each_level_evaluates_its_pinned_policy() {
        let rule_engine = RuleEngine::for_tests();
        let mut stale = Vec::new();
        for &level in SafetyLevel::VARIANTS {
            let file = format!("{}.txt", <&str>::from(level));
            let table = policy_table(rule_engine.policy(level));
            let pinned = PINNED_POLICIES
                .iter()
                .find(|&&(pinned_level, _)| pinned_level == level)
                .map(|&(_, pinned)| pinned);
            if pinned != Some(table.as_str()) {
                if let Ok(dir) = std::env::var("TEST_UNDECLARED_OUTPUTS_DIR") {
                    std::fs::write(std::path::Path::new(&dir).join(&file), &table).unwrap();
                }
                stale.push(file);
            }
        }
        assert!(
            stale.is_empty(),
            "tests/fixtures/policy/{stale:?} differ from the evaluated policies: run scripts/bless-policy and review the diff"
        );
    }

    #[test]
    #[should_panic(expected = "a rule reads Follows")]
    fn a_rule_reading_an_underived_hydrator_panics_in_tests() {
        let viewer = viewer(VIEWER_ID);
        let candidate = candidate().build();
        let context = test_context(&viewer, &candidate)
            .hydrated_by(Hydrators::all().without(Hydrator::Follows));
        context.edge(Hydrator::Follows);
    }

    #[test]
    fn each_rule_name_wires_the_same_clauses_at_every_level() {
        const WIRED_DIFFERENTLY: [&str; 6] = [
            "nsfw_high_precision/blur/nudity",
            "nsfw_high_precision/blur/sensitive",
            "gore_and_violence_high_precision/blur",
            "nsfw_card_image/blur/sensitive",
            "nsfw_admin/blur/sensitive",
            "nsfw_user/blur/sensitive_user",
        ];
        let rule_engine = RuleEngine::for_tests();
        let mut first: Vec<(&str, SafetyLevel, Vec<&Clause>)> = Vec::new();
        for &level in SafetyLevel::VARIANTS {
            for name in rule_engine.policy(level).rule_names() {
                let clauses: Vec<&Clause> = rule_engine
                    .policy(level)
                    .clauses
                    .iter()
                    .filter(|&&(other, _)| other == name)
                    .map(|(_, clause)| clause)
                    .collect();
                match first.iter().find(|(other, ..)| *other == name) {
                    None => first.push((name, level, clauses)),
                    Some((_, first_level, wired)) => assert!(
                        *wired == clauses || WIRED_DIFFERENTLY.contains(&name),
                        "{name} wires other clauses at {level:?} than at {first_level:?}: \
                         wire the same clauses at every level, or give the {level:?} \
                         clauses their own RuleId"
                    ),
                }
            }
        }
    }

    #[test]
    fn each_clause_of_a_rule_id_names_the_same_labels() {
        let mut first: Vec<(RuleId, String, Vec<String>)> = Vec::new();
        for &level in SafetyLevel::VARIANTS {
            for &clause in level.spec().rules {
                let mut labels: Vec<String> = clause
                    .when
                    .iter()
                    .filter_map(|condition| condition.own_tweet_label())
                    .collect();
                labels.sort_unstable();
                let name = clause.name();
                match first.iter().find(|(id, ..)| *id == clause.id) {
                    None => first.push((clause.id, name, labels)),
                    Some((_, other, read)) => assert!(
                        *read == labels,
                        "{name} names the labels {labels:?} but {other}, a clause of the same \
                         RuleId, names {read:?}: copy the label conditions of {other}, or give it \
                         its own RuleId"
                    ),
                }
            }
        }
    }

    #[test]
    fn each_const_a_level_lists_is_named_after_its_clause() {
        let mut by_name: Vec<(String, Vec<&Clause>)> = Vec::new();
        for &level in SafetyLevel::VARIANTS {
            for &clause in level.spec().rules {
                let name = clause.name();
                match by_name.iter_mut().find(|(other, _)| *other == name) {
                    Some((_, clauses)) if !clauses.contains(&clause) => clauses.push(clause),
                    Some(_) => {}
                    None => by_name.push((name, vec![clause])),
                }
            }
        }
        let shared = |name: &str| {
            by_name
                .iter()
                .any(|(other, clauses)| other == name && clauses.len() > 1)
        };
        for &level in SafetyLevel::VARIANTS {
            let level_name = <&str>::from(level).to_uppercase();
            let &(_, listed) = levels::LISTED
                .iter()
                .find(|&&(name, _)| name == level_name)
                .unwrap_or_else(|| panic!("levels.rs lists no {level_name}"));
            for (&listed, clause) in listed.iter().zip(level.spec().rules) {
                let name = clause.name();
                let prefix = name.replace('/', "_").to_uppercase();
                let named = match listed.strip_prefix(&prefix) {
                    Some("") => true,
                    Some(suffix) => suffix.starts_with('_') && shared(&name),
                    None => false,
                };
                assert!(
                    named,
                    "{listed} holds {name}: name it {prefix}{}",
                    if shared(&name) {
                        "_<WHAT_SETS_IT_APART>"
                    } else {
                        ""
                    }
                );
            }
        }
    }

    #[test]
    fn each_fosnr_violation_has_a_label_drop_and_each_fosnr_label_drop_a_violation() {
        let rule_engine = RuleEngine::for_tests();
        let mut dropped = Vec::new();
        for &level in SafetyLevel::VARIANTS {
            for (name, clause) in &rule_engine.policy(level).clauses {
                let id: &str = clause.id.into();
                let [only] = clause.when[..] else { continue };
                let [label] = only.tweet_labels()[..] else {
                    continue;
                };
                if !id.starts_with("fosnr") {
                    continue;
                }
                assert!(
                    tweet_rules::FOSNR_AUTHOR_VIOLATIONS
                        .iter()
                        .any(|violation| violation.label == label),
                    "{name} drops a FOSNR label FOSNR_AUTHOR_VIOLATIONS does not name: add its \
                     row to FOSNR_AUTHOR_VIOLATIONS in src/rules/tweet_rules.rs"
                );
                dropped.push(label);
            }
        }
        for violation in &tweet_rules::FOSNR_AUTHOR_VIOLATIONS {
            assert!(
                dropped.contains(&violation.label),
                "FOSNR_AUTHOR_VIOLATIONS names {:?} but no level drops it: add a `const \
                 FOSNR_<LABEL>_DROP_UNDESIRABLE: Clause` reading it and list it in the levels",
                xai_safety_label_store::types::label_meta(violation.label).map(|meta| meta.ident),
            );
        }
    }

    #[test]
    fn a_notice_reads_the_labels_it_names() {
        let rule_engine = RuleEngine::for_tests();
        for &level in SafetyLevel::VARIANTS {
            for (name, clause) in &rule_engine.policy(level).clauses {
                let mut read: Vec<SafetyLabelType> = clause
                    .when
                    .iter()
                    .flat_map(|condition| condition.tweet_labels())
                    .collect();
                let mut named: Vec<SafetyLabelType> = match clause.action.spec() {
                    ActionSpec::SoftIntervention { label, .. } => vec![*label],
                    ActionSpec::Appealable(violations) => {
                        violations.iter().map(|violation| violation.label).collect()
                    }
                    ActionSpec::Drop(_)
                    | ActionSpec::Tombstone(_)
                    | ActionSpec::MediaRestriction(_)
                    | ActionSpec::LimitedEngagement(_) => continue,
                };
                read.sort_by_key(|label| label.0);
                named.sort_by_key(|label| label.0);
                assert!(
                    read == named,
                    "{name} names {:?} but its conditions read {:?}: make `when` read exactly the \
                     labels its action names",
                    named.into_iter().map(Spelled::spelled).collect::<Vec<_>>(),
                    read.into_iter().map(Spelled::spelled).collect::<Vec<_>>(),
                );
            }
        }
    }

    mod engine {
        use super::super::*;
        use crate::hydration::Hydrator;
        use crate::models::{
            DropReason, LimitedEngagementReason, MediaInterstitial, MediaRestriction,
            PostCandidate, TombstoneReason, ViewerFeatures,
        };
        use crate::rules::fixtures::{
            VIEWER_ID, allow, author_viewer, blurred, candidate, logged_out_viewer, viewer,
        };
        use crate::rules::rule_spec::Audience::{Everyone, NonAuthors, NonFollowers};
        use crate::rules::rule_spec::conditions::for_tests::{any, viewer_does_not_follow_author};
        use crate::rules::rule_spec::{
            ActionSpec, Cond, tweet_created_after, viewer_blocks_author, viewer_follows_author,
            viewer_logged_out,
        };
        use crate::rules::test_context;
        use xai_visibility_filtering::models::FilteredReason;
        use xai_x_thrift::action::InterstitialReason;

        fn clause(when: &[Cond], action: ActionSpec) -> Clause {
            Clause {
                id: RuleId::FilterAll,
                when: when.to_vec().leak(),
                action: action.into(),
                audience: Everyone,
            }
        }

        fn always(name: &'static str, action: ActionSpec) -> (&'static str, Clause) {
            (name, clause(&[], action))
        }

        const NEVER: Cond = tweet_created_after(u64::MAX);

        const DROP_SUSPENDED: ActionSpec =
            ActionSpec::Drop(DropReason::Legacy(FilteredReason::AuthorIsSuspended));
        const TOMBSTONE: ActionSpec = ActionSpec::Tombstone(TombstoneReason::LocalRegulations);
        const INTERSTITIAL_NSFW: ActionSpec =
            ActionSpec::MediaRestriction(MediaRestriction::MediaInterstitial(MediaInterstitial {
                legacy: FilteredReason::ContainNsfwMedia,
                reason: InterstitialReason::Sensitive(true),
                prompt: None,
            }));
        const INTERSTITIAL_UNSPECIFIED: ActionSpec =
            ActionSpec::MediaRestriction(MediaRestriction::MediaInterstitial(MediaInterstitial {
                legacy: FilteredReason::UnspecifiedReason,
                reason: InterstitialReason::Nudity(true),
                prompt: None,
            }));
        const LIMIT: ActionSpec =
            ActionSpec::LimitedEngagement(LimitedEngagementReason::ConversationControl);

        fn context_inputs() -> (ViewerFeatures, PostCandidate) {
            (ViewerFeatures::default(), PostCandidate::default())
        }

        fn withheld(value: Withholding, by: &'static str) -> Verdict {
            Verdict::Withheld(Decided { value, by })
        }

        fn tombstone_first() -> Policy {
            Policy::new(vec![
                always("tombstone", TOMBSTONE),
                ("never", clause(&[NEVER], DROP_SUSPENDED)),
                always("drop", DROP_SUSPENDED),
                (
                    "after_drop",
                    clause(&[viewer_follows_author()], DROP_SUSPENDED),
                ),
            ])
        }

        fn restrictions() -> Policy {
            Policy::new(vec![
                always("first_interstitial", INTERSTITIAL_NSFW),
                always("second_interstitial", INTERSTITIAL_UNSPECIFIED),
                always("first_limit", LIMIT),
                always(
                    "second_limit",
                    ActionSpec::LimitedEngagement(LimitedEngagementReason::BlockedViewer),
                ),
                always("third_limit", LIMIT),
            ])
        }

        #[test]
        fn the_most_severe_terminal_returns_before_later_rules() {
            let (viewer, candidate) = context_inputs();
            let context = test_context(&viewer, &candidate)
                .hydrated_by(Hydrators::all().without(Hydrator::Follows));

            assert_eq!(
                tombstone_first().evaluate(&context).into_verdict(),
                withheld(
                    Withholding::Drop(DropReason::Legacy(FilteredReason::AuthorIsSuspended)),
                    "drop"
                )
            );
        }

        #[test]
        #[should_panic(expected = "is wired twice")]
        fn an_identity_recurring_after_another_fails_construction() {
            Policy::new(vec![
                always("blur", INTERSTITIAL_NSFW),
                always("other_blur", INTERSTITIAL_UNSPECIFIED),
                always("blur", INTERSTITIAL_NSFW),
            ]);
        }

        #[test]
        #[should_panic(expected = "derive filter_all/blur/sensitive")]
        fn two_identities_deriving_one_name_fail_construction() {
            let blur = |legacy| {
                ActionSpec::MediaRestriction(MediaRestriction::MediaInterstitial(
                    MediaInterstitial {
                        legacy,
                        reason: InterstitialReason::Sensitive(true),
                        prompt: None,
                    },
                ))
            };
            let mut names = Vec::new();
            interned_name(
                &mut names,
                &clause(&[], blur(FilteredReason::ContainNsfwMedia)),
            );
            interned_name(
                &mut names,
                &clause(&[], blur(FilteredReason::UnspecifiedReason)),
            );
        }

        #[test]
        #[should_panic(expected = "is wired twice")]
        fn a_clause_repeated_within_its_run_fails_construction() {
            Policy::new(vec![
                always("blur", INTERSTITIAL_NSFW),
                always("blur", INTERSTITIAL_NSFW),
            ]);
        }

        #[test]
        fn the_media_slot_keeps_its_first_value_and_the_engagement_slot_each_reason_once() {
            let (viewer, candidate) = context_inputs();

            let Verdict::Shown {
                notice: None,
                media,
                engagement: Some(limit),
            } = restrictions()
                .evaluate(&test_context(&viewer, &candidate))
                .into_verdict()
            else {
                panic!("expected a limited verdict");
            };

            assert_eq!(
                media,
                Some(Decided {
                    value: MediaRestriction::MediaInterstitial(MediaInterstitial {
                        legacy: FilteredReason::ContainNsfwMedia,
                        reason: InterstitialReason::Sensitive(true),
                        prompt: None,
                    }),
                    by: "first_interstitial",
                })
            );
            assert_eq!(limit.by, "first_limit");
            assert_eq!(
                limit.value.reasons().collect::<Vec<_>>(),
                [
                    LimitedEngagementReason::ConversationControl,
                    LimitedEngagementReason::BlockedViewer,
                ]
            );
        }

        fn follows_and_blocks_failed() -> (ViewerFeatures, PostCandidate) {
            let candidate = PostCandidate {
                failed: Hydrators::of(Hydrator::Follows).with(Hydrator::Blocks),
                ..Default::default()
            };
            (ViewerFeatures::default(), candidate)
        }

        #[test]
        fn clauses_combine_in_three_values_and_unknown_resolves_to_the_default() {
            let (viewer, candidate) = follows_and_blocks_failed();
            let follows = Hydrators::of(Hydrator::Follows);
            let dropped = withheld(
                Withholding::Drop(DropReason::Legacy(FilteredReason::AuthorIsSuspended)),
                "rule",
            );
            let rows: [(&[Cond], Verdict, Hydrators); 6] = [
                (&[viewer_follows_author()], allow(), follows),
                (
                    &[viewer_follows_author(), NEVER],
                    allow(),
                    Hydrators::empty(),
                ),
                (&[viewer_does_not_follow_author()], dropped.clone(), follows),
                (
                    &[any(&[viewer_follows_author(), viewer_logged_out()])],
                    dropped,
                    Hydrators::empty(),
                ),
                (&[any(&[viewer_follows_author(), NEVER])], allow(), follows),
                (
                    &[
                        any(&[viewer_blocks_author(), viewer_logged_out()]),
                        viewer_follows_author(),
                    ],
                    allow(),
                    follows,
                ),
            ];
            for (index, (when, verdict, fail_open_defaults)) in rows.into_iter().enumerate() {
                assert_eq!(
                    Policy::new(vec![("rule", clause(when, DROP_SUSPENDED))])
                        .evaluate(&test_context(&viewer, &candidate)),
                    Evaluation::Partial {
                        verdict,
                        fail_open_defaults
                    },
                    "row {index}"
                );
            }
        }

        #[test]
        fn fail_open_defaults_are_the_unknown_clauses_that_could_have_changed_the_verdict() {
            let (viewer, candidate) = follows_and_blocks_failed();
            let follows = Hydrators::of(Hydrator::Follows);
            let unknown = |action| ("unknown", clause(&[viewer_follows_author()], action));
            let blur = blurred(InterstitialReason::Sensitive(true), "blur");
            let rows = [
                (
                    [unknown(DROP_SUSPENDED), always("drop", DROP_SUSPENDED)],
                    withheld(
                        Withholding::Drop(DropReason::Legacy(FilteredReason::AuthorIsSuspended)),
                        "drop",
                    ),
                    follows,
                ),
                (
                    [unknown(INTERSTITIAL_NSFW), always("drop", DROP_SUSPENDED)],
                    withheld(
                        Withholding::Drop(DropReason::Legacy(FilteredReason::AuthorIsSuspended)),
                        "drop",
                    ),
                    Hydrators::empty(),
                ),
                (
                    [
                        always("blur", INTERSTITIAL_NSFW),
                        unknown(INTERSTITIAL_UNSPECIFIED),
                    ],
                    blur.clone(),
                    Hydrators::empty(),
                ),
                (
                    [always("blur", INTERSTITIAL_NSFW), unknown(LIMIT)],
                    blur,
                    follows,
                ),
            ];
            for (index, (rules, verdict, fail_open_defaults)) in rows.into_iter().enumerate() {
                assert_eq!(
                    Policy::new(rules.into()).evaluate(&test_context(&viewer, &candidate)),
                    Evaluation::Partial {
                        verdict,
                        fail_open_defaults
                    },
                    "row {index}"
                );
            }
        }

        #[test]
        fn non_followers_spare_the_author_and_followers_and_fail_closed_on_follows() {
            let follows = Hydrators::of(Hydrator::Follows);
            let dropped = withheld(
                Withholding::Drop(DropReason::Legacy(FilteredReason::AuthorIsSuspended)),
                "rule",
            );
            let followed = candidate().with_edge(Hydrator::Follows).build();
            let mut follows_failed = candidate().build();
            follows_failed.failed = follows;
            let rows = [
                (
                    "author",
                    author_viewer(),
                    followed.clone(),
                    Evaluation::Complete { verdict: allow() },
                ),
                (
                    "follower",
                    viewer(VIEWER_ID),
                    followed,
                    Evaluation::Complete { verdict: allow() },
                ),
                (
                    "non-follower",
                    viewer(VIEWER_ID),
                    candidate().build(),
                    Evaluation::Complete {
                        verdict: dropped.clone(),
                    },
                ),
                (
                    "logged out",
                    logged_out_viewer(),
                    candidate().build(),
                    Evaluation::Complete {
                        verdict: dropped.clone(),
                    },
                ),
                (
                    "follows failed",
                    viewer(VIEWER_ID),
                    follows_failed,
                    Evaluation::Partial {
                        verdict: dropped,
                        fail_open_defaults: follows,
                    },
                ),
            ];
            let non_followers = Clause {
                audience: NonFollowers,
                ..clause(&[], DROP_SUSPENDED)
            };
            let spelled_out = Clause {
                audience: NonAuthors,
                ..clause(&[viewer_does_not_follow_author()], DROP_SUSPENDED)
            };
            let policy = Policy::new(vec![("rule", non_followers.clone())]);
            for (who, viewer, candidate, evaluation) in rows {
                let context = test_context(&viewer, &candidate).hydrated_by(policy.hydrators);
                assert_eq!(
                    non_followers.applies(&context),
                    spelled_out.applies(&context),
                    "{who}"
                );
                assert_eq!(policy.evaluate(&context), evaluation, "{who}");
            }
        }
    }
}
