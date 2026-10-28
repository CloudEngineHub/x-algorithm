use crate::models::{
    AuthorId, ClientCapability, NotificationKind, PlacedCandidate, Placement, RawCandidate,
    RequestViewer, RetweetMerge, TweetId,
};
use crate::params::ClientSwitches;
use crate::post_evaluator::PostBatch;
use crate::rules::{LevelError, QuoteEntries, RuleEngine, SafetyLevel, WireLevel};
use tonic::Status;
use xai_twittercontext_proto::TwitterContextViewer;
use xai_visibility_filtering_proto as vf_pb;

pub(crate) struct Request<R> {
    pub(crate) viewer: RequestViewer,
    pub(crate) rows: R,
}

pub(crate) enum Entry {
    Judged(PlacedCandidate),
    NotJudged,
}

pub(crate) struct Actors {
    pub(crate) kind: NotificationKind,
    pub(crate) recipient: u64,
    pub(crate) ids: Vec<AuthorId>,
}

impl NotificationKind {
    fn parse(kind: Option<&vf_pb::notification::Kind>) -> Result<Self, Status> {
        match kind {
            Some(vf_pb::notification::Kind::Follow(_)) => Ok(Self::Follow),
            Some(vf_pb::notification::Kind::FollowRequestAccepted(_)) => {
                Ok(Self::FollowRequestAccepted)
            }
            None => Err(Status::invalid_argument(
                "a notification has no kind VF serves",
            )),
        }
    }
}

#[derive(Clone, Copy)]
enum LevelRpc {
    FilterTweets,
    EvaluateTweets,
    FilterNotifications,
}

pub(crate) fn level(wire: WireLevel) -> Result<SafetyLevel, Status> {
    let rpc = match wire {
        WireLevel::FilterTweets(_) => LevelRpc::FilterTweets,
        WireLevel::EvaluateTweets(_) => LevelRpc::EvaluateTweets,
    };
    SafetyLevel::parse(wire).map_err(|error| refused(rpc, error))
}

pub(crate) fn notification_level(number: i32) -> Result<(SafetyLevel, NotificationKind), Status> {
    SafetyLevel::notification_level(number)
        .map_err(|error| refused(LevelRpc::FilterNotifications, error))
}

pub(crate) fn filter_tweets(
    request: vf_pb::VisibilityFilterRequest,
) -> Request<Vec<PlacedCandidate>> {
    let rows = request
        .tweets
        .iter()
        .map(|tweet| {
            PlacedCandidate::standalone(RawCandidate {
                tweet_id: TweetId(tweet.tweet_id),
                request_author_id: tweet.author_id,
            })
        })
        .collect();
    Request {
        viewer: RequestViewer::new(request.viewer_id, request.country_code),
        rows,
    }
}

pub(crate) fn evaluate_tweets(
    request: &vf_pb::EvaluateTweetsRequest,
    context: Option<TwitterContextViewer>,
    level: SafetyLevel,
) -> Request<Vec<Entry>> {
    let judges_quote_entries = matches!(level.spec().quote_entries(), QuoteEntries::Judged { .. });
    let rows = request
        .tweets
        .iter()
        .map(|tweet| {
            let placement = match &tweet.quote_context {
                None => Placement::Standalone,
                Some(quote) if judges_quote_entries => Placement::InnerQuoted {
                    outer: TweetId(quote.outer_tweet_id),
                },
                Some(_) => return Entry::NotJudged,
            };
            Entry::Judged(PlacedCandidate {
                candidate: RawCandidate {
                    tweet_id: TweetId(tweet.tweet_id),
                    request_author_id: None,
                },
                placement,
            })
        })
        .collect();
    Request {
        viewer: RequestViewer::with_context(request.viewer_id, context),
        rows,
    }
}

pub(crate) fn filter_notifications(
    request: &vf_pb::FilterNotificationsRequest,
    level: Result<(SafetyLevel, NotificationKind), Status>,
) -> Result<(SafetyLevel, Actors), Status> {
    if request.recipient_id == 0 {
        return Err(Status::invalid_argument(
            "FilterNotifications needs a recipient",
        ));
    }
    let (level, kind) = level?;
    let ids = request
        .notifications
        .iter()
        .map(|notification| {
            if NotificationKind::parse(notification.kind.as_ref())? != kind {
                return Err(Status::invalid_argument(
                    "the safety level does not serve this notification kind",
                ));
            }
            Ok(AuthorId(notification.actor_id))
        })
        .collect::<Result<_, _>>()?;
    Ok((
        level,
        Actors {
            kind,
            recipient: request.recipient_id,
            ids,
        },
    ))
}

fn refused(rpc: LevelRpc, error: LevelError) -> Status {
    match (rpc, error) {
        (LevelRpc::FilterTweets | LevelRpc::EvaluateTweets, LevelError::Unknown) => {
            Status::invalid_argument("unknown safety level")
        }
        (LevelRpc::FilterTweets | LevelRpc::EvaluateTweets, LevelError::NotServed) => {
            Status::unimplemented("safety level has no Rust policy")
        }
        (LevelRpc::FilterNotifications, LevelError::Unknown) => {
            Status::invalid_argument("not a notification safety level")
        }
        (LevelRpc::FilterNotifications, LevelError::NotServed) => {
            Status::unimplemented("FilterNotifications does not serve this safety level")
        }
    }
}

pub(crate) fn filter_tweets_batch(
    rule_engine: &RuleEngine,
    client_switches: &ClientSwitches,
    viewer: RequestViewer,
    safety_level: SafetyLevel,
    candidates: Vec<PlacedCandidate>,
) -> PostBatch {
    let client_capability = ClientCapability {
        soft_nsfw_new_user_drop: rule_engine.reads_soft_nsfw_new_user_drop(safety_level)
            && client_switches.soft_nsfw_new_user_drop(viewer.id(), viewer.country_code()),
        ..ClientCapability::default()
    };
    PostBatch {
        viewer,
        client_capability,
        safety_level,
        candidates,
        retweet_merge: RetweetMerge::Unmerged,
    }
}

pub(crate) fn evaluate_tweets_batch(
    client_switches: &ClientSwitches,
    viewer: RequestViewer,
    safety_level: SafetyLevel,
    candidates: Vec<PlacedCandidate>,
) -> PostBatch {
    let client_capability =
        client_switches.resolve(viewer.context(), viewer.id(), viewer.country_code());
    PostBatch {
        viewer,
        client_capability,
        safety_level,
        candidates,
        retweet_merge: RetweetMerge::Merged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code::{InvalidArgument, Unimplemented};
    use xai_x_thrift::safety_level::SafetyLevel as ThriftLevel;

    #[test]
    fn each_rpc_refuses_an_unknown_and_an_unserved_level_with_its_code_and_message() {
        const UNKNOWN: i32 = 9_999;
        let notification = |number| notification_level(number).map(|(level, _)| level);
        for (name, result, code, message) in [
            (
                "filter_tweets unknown",
                Err(refused(LevelRpc::FilterTweets, LevelError::Unknown)),
                InvalidArgument,
                "unknown safety level",
            ),
            (
                "filter_tweets not served",
                Err(refused(LevelRpc::FilterTweets, LevelError::NotServed)),
                Unimplemented,
                "safety level has no Rust policy",
            ),
            (
                "evaluate_tweets unknown",
                level(WireLevel::EvaluateTweets(UNKNOWN)),
                InvalidArgument,
                "unknown safety level",
            ),
            (
                "evaluate_tweets 247",
                level(WireLevel::EvaluateTweets(
                    ThriftLevel::IMMERSIVE_EXPANDED_RECOMMENDATIONS.0,
                )),
                Unimplemented,
                "safety level has no Rust policy",
            ),
            (
                "filter_notifications unknown",
                notification(UNKNOWN),
                InvalidArgument,
                "not a notification safety level",
            ),
            (
                "filter_notifications 8",
                notification(ThriftLevel::TIMELINE_HOME.0),
                Unimplemented,
                "FilterNotifications does not serve this safety level",
            ),
            (
                "filter_notifications 247",
                notification(ThriftLevel::IMMERSIVE_EXPANDED_RECOMMENDATIONS.0),
                Unimplemented,
                "FilterNotifications does not serve this safety level",
            ),
        ] {
            let status = result.expect_err(name);
            assert_eq!((status.code(), status.message()), (code, message), "{name}");
        }
    }
}
