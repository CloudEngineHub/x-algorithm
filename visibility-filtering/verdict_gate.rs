use crate::models::viewer::Client;
use crate::result_label::{state_label, NONE};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tonic::metadata::MetadataMap;
use xai_visibility_filtering::evaluated::EvaluationResult;
use xai_visibility_filtering_proto as vf_pb;
use xai_x_thrift::safety_level::SafetyLevel;

pub const GRPC_PORT: u16 = 50051;

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
    file.cases
        .iter()
        .map(|case| {
            let name = match &case.quoted_in {
                Some(outer) => format!("{} x {} quoted in {outer}", case.viewer, case.post),
                None => format!("{} x {}", case.viewer, case.post),
            };
            Ok(Case {
                name,
                viewer_id: id(&case.viewer)?,
                post_id: id(&case.post)?,
                quoted_in: case.quoted_in.as_deref().map(id).transpose()?,
                expect: case.expect.clone(),
            })
        })
        .collect()
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
    pub fn request(&self) -> vf_pb::EvaluateTweetsRequest {
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

    pub fn metadata(&self) -> anyhow::Result<MetadataMap> {
        let context = Client::Web.context(Some(self.viewer_id), Some("us"));
        let mut metadata = MetadataMap::new();
        metadata.insert(
            "twittercontext",
            xai_twittercontext::hydrate_twitter_context(&context)?,
        );
        Ok(metadata)
    }

    pub fn judge(&self, answer: &anyhow::Result<Vec<EvaluationResult>>) -> CaseLine<'_> {
        let verdict = |(class, reason)| {
            let got = VerdictFields { class, reason };
            if got == self.expect {
                CaseResult::Pass { got }
            } else {
                CaseResult::Wrong { got }
            }
        };
        let failed = |error: String| CaseResult::CallFailed { error };
        let result = match answer.as_ref().map(|results| results.first()) {
            Ok(Some(EvaluationResult::Evaluated(state))) => verdict(state_label(state)),
            Ok(Some(EvaluationResult::NotEvaluated)) => {
                verdict(("not_evaluated".to_string(), NONE.to_string()))
            }
            Ok(Some(EvaluationResult::Failed)) => failed("VF answered failed".to_string()),
            Ok(None) => failed("no result".to_string()),
            Err(err) => failed(format!("{err:#}")),
        };
        CaseLine {
            case: &self.name,
            expect: &self.expect,
            result,
        }
    }
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
            expect: fields("drop", "author_is_suspended"),
        }
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
        assert_eq!(parse_cases(&text).unwrap().len(), cases.cases.len());
    }

    #[test]
    fn parse_refuses_an_unknown_alias() {
        let file = |case: &str| format!(r#"{{"fixtures": {{"v": 1, "p": 2}}, "cases": [{case}]}}"#);
        let expect = r#""expect": {"class": "allow", "reason": "none"}"#;
        let known = format!(r#"{{"viewer": "v", "post": "p", {expect}}}"#);
        assert_eq!(parse_cases(&file(&known)).unwrap()[0].name, "v x p");
        for case in [
            format!(r#"{{"viewer": "v", "post": "q", {expect}}}"#),
            format!(r#"{{"viewer": "v", "post": "p", "quoted_in": "q", {expect}}}"#),
        ] {
            assert!(parse_cases(&file(&case)).is_err(), "{case}");
        }
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
        let pass = case.judge(&Ok(vec![suspended, EvaluationResult::Failed]));
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
                case.judge(&Ok(vec![answer])).result,
                CaseResult::Wrong { got }
            );
        }
    }

    #[test]
    fn judge_counts_an_error_no_result_or_a_failed_answer_as_a_failed_call() {
        let case = case(None);
        for answer in [
            Err(anyhow!("deadline exceeded")),
            Ok(vec![]),
            Ok(vec![EvaluationResult::Failed]),
        ] {
            assert!(case.judge(&answer).call_failed());
        }
    }
}
