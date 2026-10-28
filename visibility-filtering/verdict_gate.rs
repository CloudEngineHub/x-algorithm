use crate::models::viewer::Client;
use crate::result_label::{debug_label, state_label, Label, NONE};
use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use tonic::metadata::MetadataMap;
use tonic::transport::Channel;
use xai_visibility_filtering::evaluated::EvaluationResult;
use xai_visibility_filtering::vf_client::XaiVfClient;
use xai_visibility_filtering_proto as vf_pb;
use xai_visibility_filtering_proto::visibility_filtering_service_client::VisibilityFilteringServiceClient;
use xai_x_thrift::safety_level::SafetyLevel;

pub const GRPC_PORT: u16 = 50051;

const CALL_TIMEOUT_MS: u64 = 5_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseFile {
    fixtures: BTreeMap<String, u64>,
    cases: Vec<CaseSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseSpec {
    viewer: String,
    post: String,
    #[serde(default)]
    quoted_in: Option<String>,
    expect: VerdictFields,
    #[serde(default)]
    filter_tweets: BTreeMap<FilterLevel, VerdictFields>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
enum FilterLevel {
    TimelineHome,
    TimelineHomeRecommendations,
    ImmersiveExpandedRecommendations,
}

impl FilterLevel {
    fn proto(self) -> vf_pb::SafetyLevel {
        match self {
            Self::TimelineHome => vf_pb::SafetyLevel::TimelineHome,
            Self::TimelineHomeRecommendations => vf_pb::SafetyLevel::TimelineHomeRecommendations,
            Self::ImmersiveExpandedRecommendations => {
                vf_pb::SafetyLevel::ImmersiveExpandedRecommendations
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerdictFields {
    class: String,
    reason: String,
}

pub struct Case {
    name: String,
    viewer_id: u64,
    post_id: u64,
    quoted_in: Option<u64>,
    filter_tweets_at: Option<FilterLevel>,
    expect: VerdictFields,
}

pub fn parse_cases(json: &str) -> anyhow::Result<Vec<Case>> {
    let file: CaseFile = serde_json::from_str(json)?;
    let id = |alias: &str| {
        file.fixtures
            .get(alias)
            .copied()
            .ok_or_else(|| anyhow!("no fixture {alias}"))
    };
    let mut calls = Vec::new();
    for case in &file.cases {
        let name = match &case.quoted_in {
            Some(outer) => format!("{} x {} quoted in {outer}", case.viewer, case.post),
            None => format!("{} x {}", case.viewer, case.post),
        };
        if case.quoted_in.is_some() && !case.filter_tweets.is_empty() {
            bail!("{name}: FilterTweets carries no quote");
        }
        let call = |name: String, filter_tweets_at, expect: &VerdictFields| {
            anyhow::Ok(Case {
                name,
                viewer_id: id(&case.viewer)?,
                post_id: id(&case.post)?,
                quoted_in: case.quoted_in.as_deref().map(id).transpose()?,
                filter_tweets_at,
                expect: expect.clone(),
            })
        };
        calls.push(call(name.clone(), None, &case.expect)?);
        for (level, expect) in &case.filter_tweets {
            let on = format!("{name} on filter_tweets/{}", <&str>::from(level));
            calls.push(call(on, Some(*level), expect)?);
        }
    }
    Ok(calls)
}

pub enum Answer {
    EvaluateTweets(anyhow::Result<Vec<EvaluationResult>>),
    FilterTweets(anyhow::Result<vf_pb::VisibilityFilterResponse>),
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
enum CaseResult {
    Pass { got: VerdictFields },
    Wrong { got: VerdictFields },
    CallFailed { error: String },
}

#[derive(Serialize)]
pub struct CaseLine<'a> {
    case: &'a str,
    expect: &'a VerdictFields,
    #[serde(flatten)]
    result: CaseResult,
}

impl Case {
    pub async fn ask(&self, channel: &Channel) -> Answer {
        let Some(level) = self.filter_tweets_at else {
            return Answer::EvaluateTweets(
                async {
                    XaiVfClient::from_channel(channel.clone())
                        .with_timeout_ms(CALL_TIMEOUT_MS)
                        .evaluate_tweets(self.request(), &self.metadata()?)
                        .await
                }
                .await,
            );
        };
        Answer::FilterTweets(
            async {
                let mut request = tonic::Request::new(self.filter_request(level));
                *request.metadata_mut() = self.metadata()?;
                request.set_timeout(Duration::from_millis(CALL_TIMEOUT_MS));
                let response = VisibilityFilteringServiceClient::new(channel.clone())
                    .filter_tweets(request)
                    .await?;
                anyhow::Ok(response.into_inner())
            }
            .await,
        )
    }

    fn request(&self) -> vf_pb::EvaluateTweetsRequest {
        let post = vf_pb::TweetData {
            tweet_id: self.post_id,
            quote_context: self
                .quoted_in
                .map(|outer_tweet_id| vf_pb::QuoteContext { outer_tweet_id }),
        };
        let outer = self.quoted_in.map(|tweet_id| vf_pb::TweetData {
            tweet_id,
            quote_context: None,
        });
        vf_pb::EvaluateTweetsRequest {
            safety_level: SafetyLevel::TIMELINE_HOME_HYDRATION.0,
            tweets: std::iter::once(post).chain(outer).collect(),
            viewer_id: Some(self.viewer_id),
        }
    }

    fn filter_request(&self, level: FilterLevel) -> vf_pb::VisibilityFilterRequest {
        vf_pb::VisibilityFilterRequest {
            safety_level: level.proto().into(),
            tweets: vec![vf_pb::TweetInput {
                tweet_id: self.post_id,
                author_id: None,
            }],
            viewer_id: Some(self.viewer_id),
            country_code: Some("US".to_string()),
        }
    }

    fn metadata(&self) -> anyhow::Result<MetadataMap> {
        let context = Client::Web.context(Some(self.viewer_id), Some("us"));
        let mut metadata = MetadataMap::new();
        metadata.insert(
            "twittercontext",
            xai_twittercontext::hydrate_twitter_context(&context)?,
        );
        Ok(metadata)
    }

    pub fn judge(&self, answer: &Answer) -> CaseLine<'_> {
        let verdict = |(class, reason): Label| {
            let got = VerdictFields { class, reason };
            if got == self.expect {
                CaseResult::Pass { got }
            } else {
                CaseResult::Wrong { got }
            }
        };
        let failed = |error: String| CaseResult::CallFailed { error };
        let result = match answer {
            Answer::EvaluateTweets(answer) => {
                match answer.as_ref().map(|results| results.first()) {
                    Ok(Some(EvaluationResult::Evaluated(state))) => verdict(state_label(state)),
                    Ok(Some(EvaluationResult::NotEvaluated)) => {
                        verdict(("not_evaluated".to_string(), NONE.to_string()))
                    }
                    Ok(Some(EvaluationResult::Failed)) => failed("VF answered failed".to_string()),
                    Ok(None) => failed("no result".to_string()),
                    Err(err) => failed(format!("{err:#}")),
                }
            }
            Answer::FilterTweets(answer) => match answer.as_ref().map(|r| r.results.first()) {
                Ok(Some(result)) => verdict(filter_label(result)),
                Ok(None) => failed("no result".to_string()),
                Err(err) => failed(format!("{err:#}")),
            },
        };
        CaseLine {
            case: &self.name,
            expect: &self.expect,
            result,
        }
    }
}

fn filter_label(result: &vf_pb::TweetVisibilityResult) -> Label {
    let kind = result.action.and_then(|action| action.kind);
    let reason = result
        .filtered_reason
        .as_ref()
        .and_then(|r| r.reason.as_ref());
    (
        kind.map_or_else(|| NONE.to_string(), |kind| debug_label(&kind).0),
        reason.map_or_else(|| NONE.to_string(), |reason| debug_label(reason).0),
    )
}

impl CaseLine<'_> {
    pub fn call_failed(&self) -> bool {
        matches!(self.result, CaseResult::CallFailed { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_x_thrift::get_tweet_fields::TweetFieldsResultState;
    use xai_x_thrift::safety_result::FilteredReason;
    use xai_x_thrift::tweet_service::TweetFieldsResultFiltered;

    fn read(path: &str) -> String {
        std::fs::read_to_string(crate::pinned::crate_path(path)).unwrap()
    }

    #[derive(Deserialize)]
    struct Fixtures {
        users: Vec<Fixture>,
        tweets: Vec<Fixture>,
    }

    #[derive(Deserialize)]
    struct Fixture {
        id: u64,
        alias: Option<String>,
    }

    fn fields(class: &str, reason: &str) -> VerdictFields {
        VerdictFields {
            class: class.to_string(),
            reason: reason.to_string(),
        }
    }

    fn case(quoted_in: Option<u64>) -> Case {
        Case {
            name: "viewer x post".to_string(),
            viewer_id: 1,
            post_id: 2,
            quoted_in,
            filter_tweets_at: None,
            expect: fields("drop", "author_is_suspended"),
        }
    }

    fn evaluated(results: Vec<EvaluationResult>) -> Answer {
        Answer::EvaluateTweets(Ok(results))
    }

    fn filtered(reason: FilteredReason) -> EvaluationResult {
        EvaluationResult::Evaluated(Box::new(TweetFieldsResultState::Filtered(
            TweetFieldsResultFiltered::new(reason),
        )))
    }

    #[test]
    fn each_case_names_fixtures_with_their_fixtures_json_ids() {
        let text = read("tests/e2e/verdict-gate.json");
        let cases: CaseFile = serde_json::from_str(&text).unwrap();
        let fixtures: Fixtures = serde_json::from_str(&read("tests/e2e/fixtures.json")).unwrap();
        let ids: BTreeMap<String, u64> = fixtures
            .users
            .into_iter()
            .chain(fixtures.tweets)
            .filter_map(|fixture| Some((fixture.alias?, fixture.id)))
            .collect();
        for (alias, id) in &cases.fixtures {
            assert_eq!(ids.get(alias), Some(id), "{alias}");
        }
        parse_cases(&text).unwrap();
    }

    #[test]
    fn parse_refuses_an_unknown_alias_and_filter_tweets_on_a_quote() {
        let file = |case: &str| format!(r#"{{"fixtures": {{"v": 1, "p": 2}}, "cases": [{case}]}}"#);
        let expect = r#""expect": {"class": "allow", "reason": "none"}"#;
        let filter = r#""filter_tweets": {"timeline_home": {"class": "allow", "reason": "none"}}"#;
        let known = format!(r#"{{"viewer": "v", "post": "p", {expect}}}"#);
        assert_eq!(parse_cases(&file(&known)).unwrap()[0].name, "v x p");
        for case in [
            format!(r#"{{"viewer": "v", "post": "q", {expect}}}"#),
            format!(r#"{{"viewer": "v", "post": "p", "quoted_in": "q", {expect}}}"#),
            format!(r#"{{"viewer": "v", "post": "p", "quoted_in": "p", {expect}, {filter}}}"#),
            format!(
                r#"{{"viewer": "v", "post": "p", {expect}, "filter_tweets": {{"timeline_home_hydration": {{"class": "allow", "reason": "none"}}}}}}"#
            ),
        ] {
            assert!(parse_cases(&file(&case)).is_err(), "{case}");
        }
    }

    #[test]
    fn parse_adds_a_filter_tweets_call_per_listed_level() {
        let text = r#"{"fixtures": {"v": 1, "p": 2}, "cases": [{"viewer": "v", "post": "p",
            "expect": {"class": "allow", "reason": "none"},
            "filter_tweets": {"timeline_home_recommendations": {"class": "drop", "reason": "contain_nsfw_media"},
                              "timeline_home": {"class": "allow", "reason": "none"}}}]}"#;
        let calls = parse_cases(text).unwrap();
        let named: Vec<_> = calls
            .iter()
            .map(|call| (call.name.as_str(), call.expect.class.as_str()))
            .collect();
        assert_eq!(
            named,
            [
                ("v x p", "allow"),
                ("v x p on filter_tweets/timeline_home", "allow"),
                (
                    "v x p on filter_tweets/timeline_home_recommendations",
                    "drop"
                ),
            ]
        );
        let request = calls[2].filter_request(calls[2].filter_tweets_at.unwrap());
        assert_eq!(
            (
                request.safety_level,
                request.viewer_id,
                request.tweets[0].tweet_id
            ),
            (
                vf_pb::SafetyLevel::TimelineHomeRecommendations.into(),
                Some(1),
                2
            )
        );
    }

    #[test]
    fn request_sends_level_82_with_a_quote_s_outer_after_the_post() {
        assert_eq!(case(None).request().safety_level, 82);
        let tweets = case(Some(3)).request().tweets;
        let sent: Vec<_> = tweets
            .iter()
            .map(|tweet| {
                (
                    tweet.tweet_id,
                    tweet.quote_context.map(|q| q.outer_tweet_id),
                )
            })
            .collect();
        assert_eq!(sent, [(2, Some(3)), (3, None)]);
    }

    #[test]
    fn judge_compares_the_verdict_fields_of_the_first_result() {
        let case = case(None);
        let suspended = filtered(FilteredReason::AuthorIsSuspended(true));
        let pass = case.judge(&evaluated(vec![suspended, EvaluationResult::Failed]));
        assert_eq!(
            pass.result,
            CaseResult::Pass {
                got: case.expect.clone()
            }
        );
        assert!(!pass.call_failed());
        for (answer, got) in [
            (
                filtered(FilteredReason::TweetIsBounced(true)),
                fields("drop", "tweet_is_bounced"),
            ),
            (
                EvaluationResult::NotEvaluated,
                fields("not_evaluated", "none"),
            ),
        ] {
            assert_eq!(
                case.judge(&evaluated(vec![answer])).result,
                CaseResult::Wrong { got }
            );
        }
    }

    #[test]
    fn judge_labels_a_filter_tweets_action_and_reason() {
        let case = case(None);
        let answer = |kind, reason: Option<vf_pb::filtered_reason::Reason>| {
            Answer::FilterTweets(Ok(vf_pb::VisibilityFilterResponse {
                results: vec![vf_pb::TweetVisibilityResult {
                    tweet_id: 2,
                    action: Some(vf_pb::Action { kind: Some(kind) }),
                    filtered_reason: reason.map(|reason| vf_pb::FilteredReason {
                        reason: Some(reason),
                    }),
                    safety_labels: None,
                }],
            }))
        };
        let drop = vf_pb::action::Kind::Drop(vf_pb::DropReason {});
        let suspended = vf_pb::filtered_reason::Reason::AuthorIsSuspended(true);
        assert_eq!(
            case.judge(&answer(drop, Some(suspended))).result,
            CaseResult::Pass {
                got: case.expect.clone()
            }
        );
        assert_eq!(
            case.judge(&answer(vf_pb::action::Kind::Allow(true), None))
                .result,
            CaseResult::Wrong {
                got: fields("allow", "none")
            }
        );
    }

    #[test]
    fn judge_counts_an_error_no_result_or_a_failed_answer_as_a_failed_call() {
        let case = case(None);
        for answer in [
            Answer::EvaluateTweets(Err(anyhow!("deadline exceeded"))),
            evaluated(vec![]),
            evaluated(vec![EvaluationResult::Failed]),
            Answer::FilterTweets(Err(anyhow!("deadline exceeded"))),
            Answer::FilterTweets(Ok(vf_pb::VisibilityFilterResponse { results: vec![] })),
        ] {
            assert!(case.judge(&answer).call_failed());
        }
    }
}
