use crate::hydration::request_context;
use crate::hydration::sources::Id;
use crate::models::viewer::Client;
use crate::models::{ClientCapability, Placement, TweetId};
use crate::post_evaluator::{PostBatch, PostEvaluator};
use crate::quote::{QuotePassMode, QuotePassRunner};
use crate::result_label::{Label, quote_label};
use crate::rules::SafetyLevel;
use crate::staging::recording::Recording;
use crate::staging::reference::tweetypie::vf_label;
use crate::treatment;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use tokio::time::Instant;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Case {
    pub(crate) id: String,
    pub(crate) tags: Vec<String>,
    pub(crate) captured_at_unix: u64,
    pub(crate) build: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) viewer_id: Option<u64>,
    pub(crate) country_code: Option<String>,
    #[serde(default)]
    pub(crate) client: Client,
    pub(crate) client_capability: ClientCapability,
    pub(crate) tweets: Vec<Expected>,
    pub(crate) recording: Recording,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Expected {
    pub(crate) tweet_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) quote_context: Option<QuoteContext>,
        pub(crate) tags: Vec<String>,
    pub(crate) tweetypie: Label,
        pub(crate) vf: Label,
    pub(crate) vf_rule: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct QuoteContext {
    pub(crate) outer_tweet_id: u64,
}

pub(crate) fn placement(quote_context: Option<QuoteContext>) -> Placement {
    match quote_context {
        None => Placement::Standalone,
        Some(quote) => Placement::InnerQuoted {
            outer: TweetId(quote.outer_tweet_id),
        },
    }
}

pub(crate) struct Evaluated {
    pub(crate) tweet_id: u64,
    pub(crate) placement: Placement,
    pub(crate) label: Label,
    pub(crate) rule: Option<(SafetyLevel, &'static str)>,
}

pub(crate) struct Fixtures {
    pub(crate) users: BTreeSet<u64>,
    pub(crate) tweets: BTreeSet<u64>,
    pub(crate) communities: BTreeSet<u64>,
    pub(crate) articles: BTreeSet<u64>,
    pub(crate) places: BTreeSet<u64>,
}

impl Fixtures {
    pub(crate) fn load(path: &Path) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct File {
            users: Vec<Fixture>,
            tweets: Vec<Fixture>,
            #[serde(default)]
            communities: Vec<Fixture>,
            #[serde(default)]
            articles: Vec<Fixture>,
            #[serde(default)]
            places: Vec<Fixture>,
        }
        #[derive(Deserialize)]
        struct Fixture {
            id: u64,
        }
        let text = fs::read_to_string(path).with_context(|| path.display().to_string())?;
        let file: File = serde_json::from_str(&text).with_context(|| path.display().to_string())?;
        let ids = |fixtures: Vec<Fixture>| fixtures.into_iter().map(|f| f.id).collect();
        Ok(Self {
            users: ids(file.users),
            tweets: ids(file.tweets),
            communities: ids(file.communities),
            articles: ids(file.articles),
            places: ids(file.places),
        })
    }

    pub(crate) fn strangers(&self, recording: &Recording) -> anyhow::Result<Vec<String>> {
        let mut strangers: Vec<String> = recording
            .ids()?
            .into_iter()
            .filter_map(|id| {
                let (kind, id, fixtures) = match id {
                    Id::User(id) => ("user", id, &self.users),
                    Id::Tweet(id) => ("tweet", id, &self.tweets),
                    Id::Community(id) => ("community", id, &self.communities),
                    Id::Article(id) => ("article", id, &self.articles),
                    Id::Place(id) => ("place", id, &self.places),
                };
                (!fixtures.contains(&id)).then(|| format!("{kind} {id}"))
            })
            .collect();
        strangers.sort();
        strangers.dedup();
        Ok(strangers)
    }
}

pub(crate) async fn evaluate(post_evaluator: &PostEvaluator, request: PostBatch) -> Vec<Evaluated> {
    let level = request.safety_level;
    let (outcomes, passes) = request_context(Instant::now(), None)
        .scope(async {
            let (outcomes, pending) = QuotePassRunner::new(QuotePassMode::Shadow)
                .serve(post_evaluator, request)
                .await;
            let passes = pending.run(post_evaluator, &outcomes).await;
            (outcomes, passes)
        })
        .await;
    outcomes
        .iter()
        .map(|outcome| {
            let served = vf_label(outcome, level);
            let rule =
                treatment::decided_by(outcome.evaluation.verdict()).map(|rule| (level, rule));
            let (label, rule) = match passes.ran().iter().find(|pass| pass.key == outcome.key()) {
                None => (served, rule),
                Some(pass) => (
                    quote_label(pass.outcome.rendering(), served),
                    pass.decided_by(rule),
                ),
            };
            Evaluated {
                tweet_id: outcome.tweet_id.0,
                placement: outcome.placement,
                label,
                rule,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Rpc;
    use crate::models::PlacedCandidate;
    use crate::params::{CountryList, CountryLists};
    use crate::pinned::crate_path;
    use crate::result_label::{Class, renders_alike};
    use crate::rules::RuleEngine;
    use crate::scenario::raw;
    use crate::staging::recording::Replay;
    use crate::staging::reference::tweetypie::SAFETY_LEVEL;
    use serde::de::DeserializeOwned;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Arc;

    const CORPUS: &str = "tests/replay_corpus";
    type Entry = (String, u64, Option<QuoteContext>);

    fn entry_name((case, tweet_id, quote): &Entry) -> String {
        match quote {
            None => format!("{case}/{tweet_id}"),
            Some(quote) => format!("{case}/{tweet_id} quoted in {}", quote.outer_tweet_id),
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Kind {
        Deliberate,
        Gap,
    }

        #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Divergence {
        divergence: String,
        kind: Kind,
        #[serde(default)]
        ticket: Option<String>,
        behavior: String,
        #[serde(default)]
        replay: Vec<Pin>,
    }

        #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Pin {
        case: String,
        tweet_id: u64,
        #[serde(default)]
        quote_context: Option<QuoteContext>,
                vf: Label,
    }

    impl Pin {
        fn entry(&self) -> Entry {
            (self.case.clone(), self.tweet_id, self.quote_context)
        }
    }

    fn registry() -> Vec<Divergence> {
        read(&crate_path("tests/divergences.json"))
    }

        fn known_divergences() -> Vec<(String, Pin)> {
        registry()
            .into_iter()
            .flat_map(|entry| {
                let name = entry.divergence;
                entry.replay.into_iter().map(move |pin| (name.clone(), pin))
            })
            .collect()
    }

                fn corpus_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("VF_REPLAY_CORPUS") {
            return dir.into();
        }
        crate_path(CORPUS)
    }

    fn read<T: DeserializeOwned>(path: &Path) -> T {
        serde_json::from_str(&fs::read_to_string(path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

        #[derive(Deserialize)]
    struct Stored {
        #[serde(flatten)]
        case: Case,
                country_lists: Option<BTreeMap<CountryList, Vec<String>>>,
    }

    fn stored(dir: &Path) -> Vec<Stored> {
        let cases = dir.join("cases");
        let mut paths: Vec<PathBuf> = fs::read_dir(&cases)
            .unwrap_or_else(|e| panic!("{}: {e}", cases.display()))
            .map(|entry| entry.unwrap().path())
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "{} holds no case", cases.display());
        paths.iter().map(|path| read(path)).collect()
    }

    fn cases(dir: &Path) -> Vec<Case> {
        stored(dir).into_iter().map(|stored| stored.case).collect()
    }

    type LevelRule = (&'static str, &'static str);

    #[derive(Clone, Copy, PartialEq, strum::IntoStaticStr)]
    #[strum(serialize_all = "snake_case")]
    enum Coverage {
        Decided,
        Diverging,
        Unreachable,
        Unlisted,
    }

    fn level_rule((level, rule): (SafetyLevel, &'static str)) -> LevelRule {
        (level.into(), rule)
    }

    struct Replayed {
        failures: Vec<String>,
                decided: BTreeSet<LevelRule>,
                diverging: BTreeSet<LevelRule>,
                mismatched: BTreeSet<Entry>,
    }

    async fn replay_cases(dir: &Path) -> Replayed {
        replay_stored(dir, stored(dir)).await
    }

    async fn replay_stored(dir: &Path, cases: Vec<Stored>) -> Replayed {
        let divergences = known_divergences();
        let shared: Recording = read(&dir.join("shared.json"));
        let shared_lists: BTreeMap<CountryList, Vec<String>> =
            read(&dir.join("country_lists.json"));
        let mut failures = Vec::new();
        let mut decided = BTreeSet::new();
        let mut diverging = BTreeSet::new();
        let mut mismatched = BTreeSet::new();
        let mut replayed = Vec::new();
        for Stored {
            case,
            country_lists,
        } in cases
        {
            replayed.extend(
                case.tweets
                    .iter()
                    .map(|t| (case.id.clone(), t.tweet_id, t.quote_context)),
            );
            let recording = case.recording.with_shared(&shared);
            let sources = match recording.into_replay(case.viewer_id) {
                Ok(sources) => Arc::new(sources),
                Err(e) => {
                    failures.push(format!("{}: {e:#}", case.id));
                    continue;
                }
            };
            let country_lists =
                CountryLists::from_codes(country_lists.as_ref().unwrap_or(&shared_lists))
                    .unwrap_or_else(|e| panic!("{}: {e}", case.id));
            let post_evaluator = PostEvaluator::new(
                Arc::<Replay>::clone(&sources),
                RuleEngine::with_country_lists(Arc::new(country_lists)),
                Arc::default(),
            );
            let request = PostBatch {
                viewer_id: case.viewer_id,
                country_code: case.country_code,
                client_capability: case.client_capability,
                safety_level: SAFETY_LEVEL,
                candidates: case
                    .tweets
                    .iter()
                    .map(|t| PlacedCandidate {
                        candidate: raw(t.tweet_id, None),
                        placement: placement(t.quote_context),
                    })
                    .collect(),
                rpc: Rpc::EvaluateTweets,
            };
            let evaluated = evaluate(&post_evaluator, request).await;
            let misses = sources.misses();
            if !misses.is_empty() {
                failures.push(format!(
                    "{}: the recording lacks {misses:?}; re-capture it with --test-users from fixtures.json",
                    case.id
                ));
                continue;
            }
            for expected in &case.tweets {
                let entry = (case.id.clone(), expected.tweet_id, expected.quote_context);
                let key = entry_name(&entry);
                let Some(vf) = evaluated.iter().find(|vf| {
                    vf.tweet_id == expected.tweet_id
                        && vf.placement == placement(expected.quote_context)
                }) else {
                    failures.push(format!("{key}: no outcome"));
                    continue;
                };
                let known = divergences.iter().find(|(_, pin)| pin.entry() == entry);
                match (renders_alike(&vf.label, &expected.tweetypie), known) {
                    (true, None) => decided.extend(vf.rule.map(level_rule)),
                    (false, Some((_, pin))) if vf.label == pin.vf => {
                        diverging.extend(vf.rule.map(level_rule));
                    }
                    (false, Some((divergence, pin))) => failures.push(format!(
                        "{key}: vf {:?} by {:?}, its known divergence pins {:?} ({divergence})",
                        vf.label, vf.rule, pin.vf
                    )),
                    (false, None) => {
                        failures.push(format!(
                            "{key} [{}; {}]: vf {:?} by {:?}, tweetypie {:?} (vf at capture {:?} by {:?})",
                            case.tags.join(","),
                            expected.tags.join(","),
                            vf.label,
                            vf.rule,
                            expected.tweetypie,
                            expected.vf,
                            expected.vf_rule,
                        ));
                        mismatched.insert(entry);
                    }
                    (true, Some((divergence, _))) => failures.push(format!(
                        "{key}: now matches tweetypie; remove its known divergence ({divergence})"
                    )),
                }
            }
        }
        failures.extend(
            divergences
                .iter()
                .map(|(_, pin)| pin.entry())
                .filter(|entry| !replayed.contains(entry))
                .map(|entry| {
                    format!(
                        "{}: known divergence names no replayed tweet",
                        entry_name(&entry)
                    )
                }),
        );
        Replayed {
            failures,
            decided,
            diverging,
            mismatched,
        }
    }

    fn decidable_rules() -> Vec<LevelRule> {
        let engine = RuleEngine::for_tests();
        let mut rules: Vec<LevelRule> = [SAFETY_LEVEL, SafetyLevel::QuotedTweetRules]
            .into_iter()
            .flat_map(|level| {
                engine
                    .wired_rule_names(level)
                    .into_iter()
                    .map(move |rule| level_rule((level, rule)))
            })
            .collect();
        rules.sort_unstable();
        rules.dedup();
        rules
    }

    #[tokio::test]
    async fn every_case_replays_to_tweetypies_answer() {
        let replayed = replay_cases(&corpus_dir()).await;
        assert!(
            replayed.failures.is_empty(),
            "\n{}",
            replayed.failures.join("\n")
        );
    }

            const UNMATCHABLE: (&str, &str) = ("unmatchable", "no_rule_renders_this");

    #[test]
    fn each_entry_is_pinned_once_and_a_gap_names_its_ticket() {
        let mut pinned = BTreeSet::new();
        for divergence in registry() {
            let name = &divergence.divergence;
            assert!(!divergence.behavior.is_empty(), "{name}: no behavior");
            if matches!(divergence.kind, Kind::Gap) {
                assert!(
                    divergence.ticket.is_some(),
                    "{name}: a gap names its ticket"
                );
            }
            for pin in &divergence.replay {
                let entry = pin.entry();
                assert!(
                    pinned.insert(entry.clone()),
                    "{}: pinned twice, again by {name}",
                    entry_name(&entry)
                );
            }
        }
    }

    #[tokio::test]
    async fn every_replayed_pair_reaches_the_comparison() {
        let dir = corpus_dir();
        let divergences = known_divergences();
        let mut cases = stored(&dir);
        let mut unpinned = BTreeSet::new();
        for stored in &mut cases {
            for tweet in &mut stored.case.tweets {
                tweet.tweetypie = (UNMATCHABLE.0.to_owned(), UNMATCHABLE.1.to_owned());
                let entry = (stored.case.id.clone(), tweet.tweet_id, tweet.quote_context);
                if !divergences.iter().any(|(_, pin)| pin.entry() == entry) {
                    unpinned.insert(entry);
                }
            }
        }

        let mismatched = replay_stored(&dir, cases).await.mismatched;

        let unreached: Vec<&Entry> = unpinned.difference(&mismatched).collect();
        assert!(
            unreached.is_empty(),
            "{} of {} unpinned pairs went unreported against a label no rule renders, first {:?}",
            unreached.len(),
            unpinned.len(),
            &unreached[..unreached.len().min(5)]
        );
    }

    #[tokio::test]
    async fn a_quote_entry_whose_tweetypie_label_flips_fails_naming_it_and_its_outer() {
        let dir = corpus_dir();
        let mut stored = stored(&dir)
            .into_iter()
            .find(|stored| stored.case.tweets.iter().any(|t| t.quote_context.is_some()))
            .expect("the corpus holds a quote entry");
        let case = stored.case.id.clone();
        let quote = stored
            .case
            .tweets
            .iter_mut()
            .find(|t| t.quote_context.is_some())
            .unwrap();
        quote.tweetypie = (UNMATCHABLE.0.to_owned(), UNMATCHABLE.1.to_owned());
        let entry = (case.clone(), quote.tweet_id, quote.quote_context);
        let named = format!("{} [", entry_name(&entry));

        let replayed = replay_stored(&dir, vec![stored]).await;

        assert_eq!(replayed.mismatched, BTreeSet::from([entry]));
        assert!(
            replayed
                .failures
                .iter()
                .any(|failure| failure.starts_with(&named)),
            "no failure names {named}: {:?}",
            replayed.failures
        );
    }

            #[tokio::test]
    async fn every_decidable_rule_is_decided_diverging_or_unreachable() {
        #[derive(Deserialize)]
        struct Unreachable {
            level: String,
            rule: String,
        }
        let replayed = replay_cases(&corpus_dir()).await;
        let unreachable: Vec<Unreachable> = read(&crate_path("scripts/corpus/unreachable.json"));
        let listed = |&(level, rule): &LevelRule| {
            unreachable
                .iter()
                .any(|u| u.level == level && u.rule == rule)
        };
        let decidable = decidable_rules();
        let state = |rule: &LevelRule| {
            if replayed.decided.contains(rule) {
                Coverage::Decided
            } else if replayed.diverging.contains(rule) {
                Coverage::Diverging
            } else if listed(rule) {
                Coverage::Unreachable
            } else {
                Coverage::Unlisted
            }
        };
        let decided = decidable
            .iter()
            .filter(|rule| state(rule) == Coverage::Decided)
            .count();
        let mut report = format!(
            "replay corpus decides {decided} of {} level-82 and level-65 rules\n",
            decidable.len()
        );
        report.extend(decidable.iter().map(|rule| {
            let (level, name) = rule;
            format!(
                "replay corpus rule {:<11} {level:<23} {name}\n",
                <&str>::from(state(rule))
            )
        }));
        std::io::stderr().write_all(report.as_bytes()).unwrap();
        let unlisted: Vec<&LevelRule> = decidable
            .iter()
            .filter(|rule| state(rule) == Coverage::Unlisted)
            .collect();
        let stale: Vec<(&str, &str)> = unreachable
            .iter()
            .map(|u| (u.level.as_str(), u.rule.as_str()))
            .filter(|rule| {
                replayed.decided.contains(rule)
                    || replayed.diverging.contains(rule)
                    || !decidable.contains(rule)
            })
            .collect();
        assert!(
            unlisted.is_empty() && stale.is_empty(),
            "rules no case decides or pins by a known divergence, missing from scripts/corpus/unreachable.json: {unlisted:?}\nunreachable.json rules a case decides, a known divergence pins or neither level wires: {stale:?}"
        );
    }

    #[test]
    fn shared_json_holds_every_viewer_free_answer_and_only_those() {
        let dir = corpus_dir();
        let shared: Recording = read(&dir.join("shared.json"));
        let (unshared, _) = shared.split_shared();
        let mut misplaced: Vec<String> = unshared
            .names()
            .iter()
            .map(|name| format!("shared.json: {name}"))
            .collect();
        for case in cases(&dir) {
            let (_, shared) = case.recording.split_shared();
            misplaced.extend(
                shared
                    .names()
                    .iter()
                    .map(|name| format!("{}: {name}", case.id)),
            );
        }
        assert!(misplaced.is_empty(), "misplaced: {misplaced:?}");
    }

    #[test]
    fn every_id_the_corpus_names_is_a_fixture() {
        let dir = corpus_dir();
        let fixtures = Fixtures::load(&dir.join("fixtures.json")).unwrap();
        let shared: Recording = read(&dir.join("shared.json"));
        let named = |file: &str, recording: &Recording| -> Vec<String> {
            fixtures
                .strangers(recording)
                .unwrap_or_else(|e| panic!("{file}: {e:#}"))
                .into_iter()
                .map(|stranger| format!("{file}: {stranger}"))
                .collect()
        };
        let mut strangers = named("shared.json", &shared);
        for case in cases(&dir) {
            strangers.extend(named(&case.id, &case.recording));
            if let Some(viewer_id) = case.viewer_id
                && !fixtures.users.contains(&viewer_id)
            {
                strangers.push(format!("{}: user {viewer_id}", case.id));
            }
            strangers.extend(
                case.tweets
                    .iter()
                    .filter(|tweet| !fixtures.tweets.contains(&tweet.tweet_id))
                    .map(|tweet| format!("{}: tweet {}", case.id, tweet.tweet_id)),
            );
        }
        assert!(strangers.is_empty(), "not in fixtures.json: {strangers:?}");
    }

            const NEVER_ALLOWED: [(&str, &str); 24] = [
        (
            "creator_exclusive_image",
            "an NSFW-labeled subscribers-only image is dropped for every viewer but its author, who sees it blurred; vfcreatr's exclusive text post is the allowed control",
        ),
        (
            "deactivated_author_post",
            "Tweetypie drops a deactivated author's post for every viewer, the author included",
        ),
        (
            "erased_author_text",
            "Tweetypie answers an erased author's post not found",
        ),
        (
            "label_fosnr_abuse",
            "FOSNR drops the post for non-followers and puts a notice on it for the author and followers",
        ),
        ("label_fosnr_abuse_insults", "as label_fosnr_abuse"),
        ("label_fosnr_civic_integrity", "as label_fosnr_abuse"),
        ("label_fosnr_hateful_conduct", "as label_fosnr_abuse"),
        ("label_fosnr_violent_speech", "as label_fosnr_abuse"),
        (
            "label_gore_and_violence_high_precision",
            "every client gets a media treatment, a tombstone or an interstitial",
        ),
        (
            "label_gore_and_violence_high_precision_image",
            "as label_gore_and_violence_high_precision, and underage, logged-out and unstated-age NSFW-country viewers drop it",
        ),
        (
            "offboarded_author_text",
            "Tweetypie drops an offboarded author's post for every viewer",
        ),
        (
            "qt_inner_deleted",
            "its author deleted it, so Tweetypie answers it not found",
        ),
        ("qt_inner_deleted_blocker", "as qt_inner_deleted"),
        ("qt_inner_erased", "as erased_author_text"),
        ("qt_inner_offboarded", "as offboarded_author_text"),
        (
            "quoted_in:qt_of_blocker",
            "the quoted author blocks the quoting author, which drops the quote for every viewer; quoted_in:qt_of_muted_followed is an allowed quote",
        ),
        (
            "quoted_in:qt_of_bounced",
            "a Bounce-labeled quote is dropped for other viewers and tombstoned for its author",
        ),
        (
            "quoted_in:qt_of_deactivated_author",
            "as deactivated_author_post, inside its quote",
        ),
        (
            "quoted_in:qt_of_deleted",
            "as qt_inner_deleted, inside its quote",
        ),
        (
            "quoted_in:qt_of_deleted_blocker",
            "the deleted quoted post's author blocks the quoting author, which drops the quote for every viewer; quoted_in:qt_of_deleted is its not-found control",
        ),
        (
            "quoted_in:qt_of_erased",
            "the quoted author is erased, which drops the quote for every viewer",
        ),
        (
            "quoted_in:qt_of_offboarded",
            "the quoted author is offboarded, which drops the quote for every viewer",
        ),
        (
            "quoted_in:qt_of_protected",
            "the quoting author does not follow the protected quoted author, which drops the quote for every viewer; quoted_in:qt_of_protected_by_follower is its allowed control",
        ),
        (
            "stale_original",
            "a superseded edit is limited for every viewer; stale_edit is its allowed control",
        ),
    ];

    #[test]
    fn every_tag_has_an_allowed_pair_or_a_reason_it_cannot() {
        let mut allowed: BTreeMap<String, bool> = BTreeMap::new();
        for case in cases(&corpus_dir()) {
            for tweet in &case.tweets {
                let allow = tweet.tweetypie.0 == <&str>::from(Class::Allow);
                for tag in &tweet.tags {
                    *allowed.entry(tag.clone()).or_default() |= allow;
                }
            }
        }
        let exempt = |tag: &str| NEVER_ALLOWED.iter().any(|(never, _)| *never == tag);
        let missing: Vec<&str> = allowed
            .iter()
            .filter(|&(tag, &allow)| !allow && !exempt(tag))
            .map(|(tag, _)| tag.as_str())
            .collect();
        let stale: Vec<&str> = NEVER_ALLOWED
            .iter()
            .map(|(tag, _)| *tag)
            .filter(|tag| allowed.get(*tag) != Some(&false))
            .collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "tags with no pair Tweetypie allows, missing from NEVER_ALLOWED: {missing:?}\nNEVER_ALLOWED tags the corpus allows or no longer has: {stale:?}"
        );
    }

    #[test]
    fn every_fixture_user_states_what_gizmoduck_answers_for_it() {
        #[derive(Deserialize)]
        struct File {
            users: Vec<User>,
        }
        #[derive(Deserialize)]
        struct User {
            id: u64,
            gizmoduck: Option<BTreeMap<String, serde::de::IgnoredAny>>,
        }
        let file: File = read(&corpus_dir().join("fixtures.json"));
        let missing: Vec<String> = file
            .users
            .iter()
            .filter(|user| user.gizmoduck.is_none())
            .map(|user| user.id.to_string())
            .collect();
        assert!(
            missing.is_empty(),
            "fixtures.json users without the gizmoduck object vf-fixture-mint --check compares: {}",
            missing.join(", ")
        );
    }
}
