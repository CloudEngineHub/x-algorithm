use crate::metrics::{self, Endpoint, NotificationKind, RequestLevel};
use crate::models::{AuthorId, Evaluation};
use crate::post_evaluator::PostEvaluator;
use crate::rules::SafetyLevel;
use crate::treatment;
use std::collections::HashMap;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use xai_visibility_filtering_proto as vf_pb;

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

    const fn of_level(level: SafetyLevel) -> Option<Self> {
        match level {
            SafetyLevel::NotificationsFollow => Some(Self::Follow),
            SafetyLevel::NotificationsWriterV2 => Some(Self::FollowRequestAccepted),
            SafetyLevel::FilterAll
            | SafetyLevel::TimelineHome
            | SafetyLevel::TimelineHomeRecommendations
            | SafetyLevel::TimelineHomeHydration
            | SafetyLevel::ImmersiveExpandedRecommendations
            | SafetyLevel::QuotedTweetRules => None,
        }
    }
}

struct Notifications {
    level: SafetyLevel,
    kind: NotificationKind,
    recipient_id: u64,
    actors: Vec<AuthorId>,
}

impl Notifications {
    fn parse(request: &vf_pb::FilterNotificationsRequest) -> Result<Self, Status> {
        if request.recipient_id == 0 {
            return Err(Status::invalid_argument(
                "FilterNotifications needs a recipient",
            ));
        }
        let (level, kind) = SafetyLevel::from_notification_thrift(request.safety_level)
            .and_then(|level| Some((level, NotificationKind::of_level(level)?)))
            .ok_or_else(|| Status::invalid_argument("not a notification safety level"))?;
        let actors = request
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
        Ok(Self {
            level,
            kind,
            recipient_id: request.recipient_id,
            actors,
        })
    }
}

pub struct FilterNotificationsEndpoint {
    post_evaluator: Arc<PostEvaluator>,
}

impl FilterNotificationsEndpoint {
    pub(crate) fn new(post_evaluator: Arc<PostEvaluator>) -> Self {
        Self { post_evaluator }
    }

    pub async fn handle(
        &self,
        request: Request<vf_pb::FilterNotificationsRequest>,
    ) -> Result<Response<vf_pb::FilterNotificationsResponse>, Status> {
        let level = RequestLevel::of(
            SafetyLevel::from_notification_thrift(request.get_ref().safety_level)
                .filter(|&level| NotificationKind::of_level(level).is_some()),
        );
        let endpoint = Endpoint::FilterNotifications;
        metrics::observe_request(endpoint, level, request, |request, context| async move {
            metrics::record_batch_size(endpoint, request.get_ref().notifications.len());
            let notifications = Notifications::parse(request.get_ref())?;
            let mut actors = notifications.actors.clone();
            actors.sort_unstable_by_key(|actor| actor.0);
            actors.dedup();
            let by_actor: HashMap<AuthorId, Option<Evaluation>> = if actors.is_empty() {
                HashMap::new()
            } else {
                context
                    .scope(self.post_evaluator.evaluate_users(
                        notifications.level,
                        notifications.recipient_id,
                        actors,
                    ))
                    .await
            };
            let evaluation = |actor: &AuthorId| by_actor.get(actor).and_then(Option::as_ref);
            metrics::record_sent(
                endpoint,
                notifications.level,
                notifications.actors.iter().filter_map(evaluation),
                notifications
                    .actors
                    .iter()
                    .filter(|&actor| evaluation(actor).is_none())
                    .count(),
            );
            let results: Vec<_> = notifications
                .actors
                .iter()
                .map(|actor| treatment::notification_result(evaluation(actor)))
                .collect();
            metrics::record_notification_results(
                notifications.kind.into(),
                results.iter().map(|&(_, labels)| labels),
            );
            Ok(Response::new(vf_pb::FilterNotificationsResponse {
                results: results.into_iter().map(|(result, _)| result).collect(),
            }))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::socialgraph_client::Graph;
    use crate::hydration::inventory::Source;
    use crate::hydration::sources::{found_user, suspended, Fault, InMemorySources};
    use crate::metrics::thread_recorded;
    use crate::rules::RuleEngine;
    use vf_pb::NotificationDropReason as DropReason;
    use xai_core_entities::entities::{
        GizmoduckAdvancedFilters, GizmoduckMuteSettings, GizmoduckUser, GizmoduckUserResult, Label,
        Labels, MentionFilter,
    };
    use xai_x_thrift::user_labels::LabelValue;

    const RECIPIENT: u64 = 50;
    const FOLLOW: i32 = 246;
    const FOLLOW_REQUEST_ACCEPTED: i32 = 72;
    const ACTOR: u64 = 1;
    const OTHER_ACTOR: u64 = 2;
    const SUSPENDED_ACTOR: u64 = 3;
    const MISSING_ACTOR: u64 = 4;

    #[derive(Debug, PartialEq, Eq)]
    enum Seen {
        Allowed,
        Dropped(DropReason),
        Failed,
    }

    fn follow(actor_id: u64) -> vf_pb::Notification {
        vf_pb::Notification {
            actor_id,
            kind: Some(vf_pb::notification::Kind::Follow(vf_pb::Follow {})),
        }
    }

    fn accepted(actor_id: u64) -> vf_pb::Notification {
        vf_pb::Notification {
            actor_id,
            kind: Some(vf_pb::notification::Kind::FollowRequestAccepted(
                vf_pb::FollowRequestAccepted {},
            )),
        }
    }

    fn recipient(filters: GizmoduckAdvancedFilters) -> GizmoduckUser {
        GizmoduckUser {
            user_id: RECIPIENT,
            mute_settings: Some(GizmoduckMuteSettings {
                muted_keywords: vec![],
                advanced_notification_filters: filters,
            }),
            ..Default::default()
        }
    }

    fn filtering(filters: GizmoduckAdvancedFilters) -> InMemorySources {
        world().viewer(RECIPIENT, recipient(filters))
    }

    fn actor_with_image(user_id: u64) -> GizmoduckUserResult {
        let mut found = found_user(user_id);
        if let Some(user) = found.user.as_mut() {
            user.profile.has_profile_image = true;
        }
        found
    }

    fn labeled(user_id: u64, label: LabelValue) -> GizmoduckUserResult {
        let mut found = actor_with_image(user_id);
        if let Some(user) = found.user.as_mut() {
            user.labels = Labels {
                labels: vec![Label {
                    label_value: label.0,
                    created_at_msec: 0,
                }],
            };
        }
        found
    }

    fn world() -> InMemorySources {
        InMemorySources::default()
            .viewer(RECIPIENT, recipient(GizmoduckAdvancedFilters::default()))
            .user(ACTOR, actor_with_image(ACTOR))
            .user(OTHER_ACTOR, actor_with_image(OTHER_ACTOR))
            .user(SUSPENDED_ACTOR, suspended())
    }

    async fn filter(
        sources: InMemorySources,
        safety_level: i32,
        recipient_id: u64,
        notifications: Vec<vf_pb::Notification>,
    ) -> (Result<Vec<Seen>, tonic::Code>, Arc<InMemorySources>) {
        let sources = Arc::new(sources);
        let endpoint = FilterNotificationsEndpoint::new(Arc::new(PostEvaluator::new(
            Arc::clone(&sources) as _,
            RuleEngine::for_tests(),
            Arc::default(),
        )));
        let response = endpoint
            .handle(Request::new(vf_pb::FilterNotificationsRequest {
                safety_level,
                recipient_id,
                notifications,
            }))
            .await;
        let seen = response
            .map(|response| {
                use vf_pb::notification_visibility_result::Outcome;
                response
                    .into_inner()
                    .results
                    .into_iter()
                    .map(|result| match result.outcome {
                        Some(Outcome::Allowed(_)) => Seen::Allowed,
                        Some(Outcome::Dropped(dropped)) => Seen::Dropped(
                            dropped
                                .reason
                                .and_then(|reason| DropReason::try_from(reason).ok())
                                .unwrap_or(DropReason::Unspecified),
                        ),
                        Some(Outcome::Failed(_)) | None => Seen::Failed,
                    })
                    .collect()
            })
            .map_err(|status| status.code());
        (seen, sources)
    }

    async fn follows(sources: InMemorySources, actors: &[u64]) -> Vec<Seen> {
        let notifications = actors.iter().copied().map(follow).collect();
        filter(sources, FOLLOW, RECIPIENT, notifications)
            .await
            .0
            .unwrap()
    }

    #[tokio::test]
    async fn invalid_requests_are_refused_and_no_notifications_answer_empty() {
        let no_kind = vf_pb::Notification {
            actor_id: ACTOR,
            kind: None,
        };
        for (level, recipient_id, notifications) in [
            (FOLLOW, 0, vec![follow(ACTOR)]),
            (FOLLOW, RECIPIENT, vec![follow(ACTOR), no_kind]),
            (
                FOLLOW,
                RECIPIENT,
                vec![follow(ACTOR), accepted(OTHER_ACTOR)],
            ),
            (FOLLOW_REQUEST_ACCEPTED, RECIPIENT, vec![follow(ACTOR)]),
            (8, RECIPIENT, vec![follow(ACTOR)]),
            (9_999, RECIPIENT, vec![follow(ACTOR)]),
        ] {
            let (seen, sources) = filter(world(), level, recipient_id, notifications).await;
            assert_eq!(seen, Err(tonic::Code::InvalidArgument), "level {level}");
            assert!(sources.calls().is_empty());
        }
        let (seen, sources) = filter(world(), FOLLOW, RECIPIENT, vec![]).await;
        assert_eq!(seen, Ok(vec![]));
        assert!(sources.calls().is_empty());
    }

    #[tokio::test]
    async fn a_level_without_notifications_counts_as_an_unknown_level_failure() {
        let requests = |outcome| {
            [
                ("rpc", "filter_notifications"),
                ("safety_level", "unknown"),
                ("outcome", outcome),
            ]
            .to_vec()
        };
        for (family, expected) in [
            (
                "vf_requests",
                vec![requests("started"), requests("failure")],
            ),
            (
                "vf_requests_by_caller",
                vec![vec![
                    ("caller_identity", "unknown"),
                    ("rpc", "filter_notifications"),
                    ("outcome", "failure"),
                ]],
            ),
        ] {
            thread_recorded::start();
            let (seen, _) = filter(world(), 8, RECIPIENT, vec![follow(ACTOR)]).await;
            assert_eq!(seen, Err(tonic::Code::InvalidArgument));
            let recorded: Vec<Vec<(String, String)>> = thread_recorded::take(family)
                .into_iter()
                .map(|sample| sample.labels)
                .collect();
            let expected: Vec<Vec<(String, String)>> = expected
                .iter()
                .map(|labels| {
                    labels
                        .iter()
                        .map(|&(label, value)| (label.to_owned(), value.to_owned()))
                        .collect()
                })
                .collect();
            assert_eq!(recorded, expected, "{family}");
        }
    }

    #[tokio::test]
    async fn each_notification_counts_in_the_verdict_families_and_unresolves_nothing() {
        let row = |value: &str, count: f64| {
            (
                value.to_owned(),
                "notifications_follow".to_owned(),
                "filter_notifications".to_owned(),
                count,
            )
        };
        for (family, label, judged, unevaluated) in [
            (
                "vf_verdicts",
                "action",
                vec![row("allow", 2.0), row("drop", 2.0)],
                vec![row("drop", 2.0)],
            ),
            (
                "vf_verdicts_by_rule",
                "rule",
                vec![row("not_found", 1.0), row("suspended_author/drop", 1.0)],
                vec![row("lookup_failed", 2.0)],
            ),
            (
                "vf_evaluated_candidates",
                "fail_open_defaults",
                vec![row("none", 4.0)],
                vec![],
            ),
            ("vf_unresolved_tweets", "lookup", vec![], vec![]),
        ] {
            for (sources, actors, expected) in [
                (
                    world(),
                    &[ACTOR, SUSPENDED_ACTOR, ACTOR, MISSING_ACTOR][..],
                    judged,
                ),
                (
                    world().fault(Source::GizmoduckViewer, Fault::Fails),
                    &[ACTOR, ACTOR],
                    unevaluated,
                ),
            ] {
                thread_recorded::start();
                follows(sources, actors).await;
                let mut recorded: Vec<(String, String, String, f64)> =
                    thread_recorded::take(family)
                        .into_iter()
                        .map(|sample| {
                            (
                                sample.label(label).unwrap().to_owned(),
                                sample.label("safety_level").unwrap().to_owned(),
                                sample.label("rpc").unwrap().to_owned(),
                                sample.value,
                            )
                        })
                        .collect();
                recorded.sort_by(|x, y| x.0.cmp(&y.0));
                assert_eq!(recorded, expected, "{family} {actors:?}");
            }
        }
    }

    #[tokio::test]
    async fn each_notification_answers_in_order_and_a_repeated_actor_costs_no_call() {
        let sources = world()
            .edge(Graph::Blocks, RECIPIENT, OTHER_ACTOR)
            .user(5, actor_with_image(5))
            .edge(Graph::Mutes, RECIPIENT, 5)
            .user(6, labeled(6, LabelValue::READ_ONLY));
        let (seen, sources) = filter(
            sources,
            FOLLOW,
            RECIPIENT,
            [
                ACTOR,
                SUSPENDED_ACTOR,
                OTHER_ACTOR,
                ACTOR,
                MISSING_ACTOR,
                5,
                6,
            ]
            .map(follow)
            .into(),
        )
        .await;
        assert_eq!(
            seen,
            Ok(vec![
                Seen::Allowed,
                Seen::Dropped(DropReason::ActorSuspended),
                Seen::Dropped(DropReason::RecipientBlocksActor),
                Seen::Allowed,
                Seen::Dropped(DropReason::ActorNotFound),
                Seen::Dropped(DropReason::RecipientMutesActor),
                Seen::Dropped(DropReason::Unspecified),
            ])
        );
        assert_eq!(
            sources.keys(Source::GizmoduckActor),
            [vec![
                ACTOR,
                OTHER_ACTOR,
                SUSPENDED_ACTOR,
                MISSING_ACTOR,
                5,
                6
            ]]
        );
        assert_eq!(sources.selects().len(), 1);
        assert_eq!(sources.keys(Source::GizmoduckViewer), [vec![RECIPIENT]]);
        for unasked in [
            Source::TesPureCoreData,
            Source::GizmoduckAuthor,
            Source::ActorEmails,
            Source::ActorPhones,
        ] {
            assert!(!sources.calls().contains(&unasked), "{unasked:?}");
        }
    }

    #[tokio::test]
    async fn only_an_accepted_follow_request_drops_a_protected_actor() {
        const PROTECTED_ACTOR: u64 = 5;
        let mut protected = actor_with_image(PROTECTED_ACTOR);
        if let Some(user) = protected.user.as_mut() {
            user.safety.is_protected = true;
        }
        let world = || world().user(PROTECTED_ACTOR, protected.clone());
        let (accepted_results, sources) = filter(
            world(),
            FOLLOW_REQUEST_ACCEPTED,
            RECIPIENT,
            [PROTECTED_ACTOR, ACTOR, SUSPENDED_ACTOR]
                .map(accepted)
                .into(),
        )
        .await;
        assert_eq!(
            accepted_results,
            Ok(vec![
                Seen::Dropped(DropReason::ActorProtected),
                Seen::Allowed,
                Seen::Dropped(DropReason::ActorSuspended),
            ])
        );
        assert_eq!(sources.calls().len(), 3);
        assert_eq!(follows(world(), &[PROTECTED_ACTOR]).await, [Seen::Allowed]);
        let (followed, _) = filter(
            world().edge(Graph::Follows, RECIPIENT, PROTECTED_ACTOR),
            FOLLOW_REQUEST_ACCEPTED,
            RECIPIENT,
            vec![accepted(PROTECTED_ACTOR)],
        )
        .await;
        assert_eq!(followed, Ok(vec![Seen::Allowed]));
    }

    #[tokio::test]
    async fn a_muted_actor_the_recipient_follows_is_kept() {
        let sources =
            world()
                .edge(Graph::Mutes, RECIPIENT, ACTOR)
                .edge(Graph::Follows, RECIPIENT, ACTOR);
        assert_eq!(follows(sources, &[ACTOR]).await, [Seen::Allowed]);
    }

    #[tokio::test]
    async fn not_followed_by_drops_only_an_actor_neither_following_nor_followed() {
        let filters = GizmoduckAdvancedFilters {
            filter_not_followed_by: true,
            ..Default::default()
        };
        let sources = filtering(filters)
            .user(5, actor_with_image(5))
            .edge(Graph::Follows, OTHER_ACTOR, RECIPIENT)
            .edge(Graph::Follows, RECIPIENT, 5);
        assert_eq!(
            follows(sources, &[ACTOR, OTHER_ACTOR, 5]).await,
            [
                Seen::Dropped(DropReason::Unspecified),
                Seen::Allowed,
                Seen::Allowed
            ]
        );
    }

    #[tokio::test]
    async fn email_and_phone_lookups_run_only_for_a_recipient_filtering_on_them() {
        for (filters, source) in [
            (
                GizmoduckAdvancedFilters {
                    filter_no_confirmed_email: true,
                    ..Default::default()
                },
                Source::ActorEmails,
            ),
            (
                GizmoduckAdvancedFilters {
                    filter_no_confirmed_phone: true,
                    ..Default::default()
                },
                Source::ActorPhones,
            ),
        ] {
            let sources = filtering(filters)
                .confirmed_email(ACTOR)
                .verified_phone(ACTOR)
                .edge(Graph::Follows, RECIPIENT, 5)
                .user(5, actor_with_image(5));
            let (seen, sources) = filter(
                sources,
                FOLLOW,
                RECIPIENT,
                [ACTOR, OTHER_ACTOR, OTHER_ACTOR, 5].map(follow).into(),
            )
            .await;
            assert_eq!(
                seen,
                Ok(vec![
                    Seen::Allowed,
                    Seen::Dropped(DropReason::Unspecified),
                    Seen::Dropped(DropReason::Unspecified),
                    Seen::Allowed,
                ]),
                "{source:?}"
            );
            assert_eq!(sources.keys(source), [vec![ACTOR, OTHER_ACTOR, 5]]);
            assert_eq!(sources.calls().len(), 4, "{source:?}");
        }
        let (_, unfiltered) = filter(world(), FOLLOW, RECIPIENT, vec![follow(ACTOR)]).await;
        assert_eq!(unfiltered.calls().len(), 3);
    }

    #[tokio::test]
    async fn profile_age_and_mention_filters_drop_a_non_follower() {
        let fresh = || {
            let mut user = actor_with_image(5);
            if let Some(user) = user.user.as_mut() {
                user.created_at_msec = i64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis(),
                )
                .ok();
            }
            user
        };
        for (filters, user, mention_filter) in [
            (
                GizmoduckAdvancedFilters {
                    filter_new_users: true,
                    ..Default::default()
                },
                (5, fresh()),
                None,
            ),
            (
                GizmoduckAdvancedFilters {
                    filter_default_profile_image: true,
                    ..Default::default()
                },
                (6, found_user(6)),
                None,
            ),
            (
                GizmoduckAdvancedFilters::default(),
                (ACTOR, actor_with_image(ACTOR)),
                Some(MentionFilter::Following),
            ),
        ] {
            let mut viewer = recipient(filters);
            viewer.account.mention_filter = mention_filter;
            let (id, user) = user;
            let sources = world().viewer(RECIPIENT, viewer).user(id, user);
            assert_eq!(
                follows(sources, &[id]).await,
                [Seen::Dropped(DropReason::Unspecified)],
                "{id}"
            );
        }
    }

    #[tokio::test]
    async fn recipient_state_and_labels_decode_as_scala_reads_them() {
        let mut suspended_recipient = recipient(GizmoduckAdvancedFilters::default());
        suspended_recipient.safety.suspended = true;
        let mut blink_recipient = recipient(GizmoduckAdvancedFilters::default());
        blink_recipient.labels = labeled(RECIPIENT, LabelValue::BLINK_BAD)
            .user
            .map(|user| user.labels)
            .unwrap_or_default();
        let blink_worst = || world().user(5, labeled(5, LabelValue::BLINK_WORST));
        for (name, sources, actor, expected) in [
            (
                "suspended recipient",
                world().viewer(RECIPIENT, suspended_recipient),
                ACTOR,
                Seen::Dropped(DropReason::Unspecified),
            ),
            (
                "blink worst actor, recipient with the omitted UQF setting",
                blink_worst(),
                5,
                Seen::Dropped(DropReason::Unspecified),
            ),
            (
                "blink worst actor, blink labeled recipient",
                blink_worst().viewer(RECIPIENT, blink_recipient),
                5,
                Seen::Allowed,
            ),
        ] {
            assert_eq!(follows(sources, &[actor]).await, [expected], "{name}");
        }
    }

    #[tokio::test]
    async fn failed_lookups_fail_only_the_notifications_whose_verdict_read_them() {
        let email_filter = GizmoduckAdvancedFilters {
            filter_no_confirmed_email: true,
            ..Default::default()
        };
        for (name, sources, expected) in [
            (
                "actor",
                world().fail_key(Source::GizmoduckActor, ACTOR),
                [Seen::Failed, Seen::Allowed],
            ),
            (
                "partial actor",
                world().fault(Source::GizmoduckActor, Fault::AnswersPartial),
                [Seen::Failed, Seen::Failed],
            ),
            (
                "recipient",
                world().fault(Source::GizmoduckViewer, Fault::Fails),
                [Seen::Failed, Seen::Failed],
            ),
            (
                "email",
                filtering(email_filter.clone())
                    .confirmed_email(OTHER_ACTOR)
                    .fail_key(Source::ActorEmails, ACTOR),
                [Seen::Failed, Seen::Allowed],
            ),
            (
                "unread email",
                world().fault(Source::ActorEmails, Fault::Fails),
                [Seen::Allowed, Seen::Allowed],
            ),
            (
                "flock",
                world().fail_graph(Graph::Blocks),
                [Seen::Failed, Seen::Failed],
            ),
        ] {
            assert_eq!(
                follows(sources, &[ACTOR, OTHER_ACTOR]).await,
                expected,
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn a_recipient_gizmoduck_does_not_find_evaluates_as_a_default_viewer() {
        let sources = InMemorySources::default()
            .user(ACTOR, actor_with_image(ACTOR))
            .user(SUSPENDED_ACTOR, suspended());
        assert_eq!(
            follows(sources, &[ACTOR, SUSPENDED_ACTOR]).await,
            [Seen::Allowed, Seen::Dropped(DropReason::ActorSuspended)]
        );
    }
}
