use crate::hydration::batch::Completeness;
use crate::hydration::metrics::{record_hydrator_request, HydratorOutcome};
use crate::models::{ViewerAge, ViewerProfile};
use crate::rules::SafetyLevel;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;
use xai_core_entities::entities::VerifiedType;
use xai_core_entities::gizmoduck_client::{GizmoduckClient, QueryFields, ViewerData};
use xai_x_rpc::WithBudget;

const CLIENT_TIMEOUT: Duration = crate::hydration::HYDRATION_TIMEOUT;

const VIEWER_QUERY_FIELDS: [QueryFields; 3] = [
    QueryFields::ACCOUNT,
    QueryFields::EXTENDED_PROFILE,
    QueryFields::SAFETY,
];

fn has_verified_badge(data: &ViewerData) -> bool {
    matches!(
        data.verified_type,
        Some(VerifiedType::Business | VerifiedType::Government)
    ) || data.is_blue_verified
}

pub struct ViewerHydrator {
    pub gizmoduck_client: Arc<dyn GizmoduckClient + Send + Sync>,
}

impl ViewerHydrator {
    pub async fn hydrate(&self, id: u64, safety_level: SafetyLevel) -> Completeness<ViewerProfile> {
        let start = Instant::now();
        let result = self
            .gizmoduck_client
            .get_viewer_data_with_fields(id, &VIEWER_QUERY_FIELDS)
            .with_budget(CLIENT_TIMEOUT)
            .await;
        let outcome = match &result {
            Ok(Ok(_)) => HydratorOutcome::Success,
            Ok(Err(_)) => HydratorOutcome::Error,
            Err(_) => HydratorOutcome::Timeout,
        };
        record_hydrator_request(
            "gizmoduck",
            "get_viewer_data",
            safety_level,
            outcome,
            1,
            start.elapsed().as_secs_f64() * 1000.0,
        );
        match result {
            Ok(Ok(data)) => {
                let viewer_age = match data.age_in_years {
                    Some(age) => ViewerAge::Known(age),
                    None if data.user_exists => ViewerAge::NotStated,
                    None => ViewerAge::Unknown,
                };
                Completeness::Complete(ViewerProfile {
                    allows_sensitive_media: data.nsfw_view.unwrap_or(false),
                    viewer_age,
                    has_verified_badge: has_verified_badge(&data),
                    account_country_code: data.account_country_code.map(|c| c.to_ascii_lowercase()),
                })
            }
            Ok(Err(e)) => {
                warn!(error = %e, "Gizmoduck viewer lookup failed; failing open");
                Completeness::Incomplete(ViewerProfile::default())
            }
            Err(_) => {
                warn!("Gizmoduck viewer lookup timed out; failing open");
                Completeness::Incomplete(ViewerProfile::default())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::collections::HashMap;
    use xai_core_entities::entities::{GizmoduckUserResult, PCFLabel};
    use xai_core_entities::gizmoduck_client::{MockGizmoduckClient, UserFields};

    fn hydrator_with_viewer_data(user_id: u64, data: ViewerData) -> ViewerHydrator {
        let mut client = MockGizmoduckClient::default();
        client.viewer_data.insert(user_id, data);
        ViewerHydrator {
            gizmoduck_client: Arc::new(client),
        }
    }

    enum ViewerLookup {
        Fails,
        Hangs,
    }

    struct BrokenViewerClient(ViewerLookup);

    #[tonic::async_trait]
    impl GizmoduckClient for BrokenViewerClient {
        async fn get_viewer_data_with_fields(
            &self,
            _user_id: u64,
            _query_fields: &[QueryFields],
        ) -> Result<ViewerData> {
            match self.0 {
                ViewerLookup::Fails => Err(anyhow::anyhow!("gizmoduck unavailable")),
                ViewerLookup::Hangs => std::future::pending().await,
            }
        }

        async fn get_users(
            &self,
            _user_ids: Vec<i64>,
        ) -> HashMap<i64, Result<Option<GizmoduckUserResult>>> {
            unreachable!()
        }

        async fn get_users_with_perspective(
            &self,
            _viewer_id: i64,
            _user_ids: Vec<i64>,
        ) -> HashMap<i64, Result<Option<GizmoduckUserResult>>> {
            unreachable!()
        }

        async fn get_viewer_roles(&self, _user_id: u64) -> Result<Vec<String>> {
            unreachable!()
        }

        async fn get_viewer_data(&self, _user_id: u64) -> Result<ViewerData> {
            unreachable!()
        }

        async fn get_pcf_labels(&self, _user_ids: Vec<i64>) -> HashMap<i64, Result<PCFLabel>> {
            unreachable!()
        }

        async fn get_profile_description_languages(
            &self,
            _user_ids: Vec<i64>,
        ) -> HashMap<i64, Result<Option<String>>> {
            unreachable!()
        }

        async fn get_user_fields(&self, _user_ids: Vec<i64>) -> HashMap<i64, Result<UserFields>> {
            unreachable!()
        }

        async fn get_by_screen_name(
            &self,
            _screen_name: &str,
        ) -> Result<Option<GizmoduckUserResult>> {
            unreachable!()
        }
    }

    async fn hydrate_with_broken_client(lookup: ViewerLookup) -> ViewerProfile {
        let hydrator = ViewerHydrator {
            gizmoduck_client: Arc::new(BrokenViewerClient(lookup)),
        };
        let Completeness::Incomplete(profile) = hydrator.hydrate(123, SafetyLevel::FilterAll).await
        else {
            panic!("broken viewer lookups are incomplete")
        };
        profile
    }

    #[tokio::test]
    async fn rpc_error_fails_open_to_the_default_profile() {
        let profile = hydrate_with_broken_client(ViewerLookup::Fails).await;

        assert!(!profile.allows_sensitive_media);
        assert_eq!(profile.viewer_age, ViewerAge::Unknown);
        assert_eq!(profile.account_country_code, None);
        assert!(!profile.has_verified_badge);
    }

    #[tokio::test(start_paused = true)]
    async fn rpc_timeout_fails_open_to_the_default_profile() {
        let profile = hydrate_with_broken_client(ViewerLookup::Hangs).await;

        assert!(!profile.allows_sensitive_media);
        assert_eq!(profile.viewer_age, ViewerAge::Unknown);
        assert_eq!(profile.account_country_code, None);
        assert!(!profile.has_verified_badge);
    }

    #[tokio::test]
    async fn viewer_existence_and_preference_determine_age_and_sensitive_media() {
        for (data, expected_age, expected_sensitive_media) in [
            (
                ViewerData {
                    user_exists: true,
                    nsfw_view: Some(false),
                    age_in_years: None,
                    ..Default::default()
                },
                ViewerAge::NotStated,
                false,
            ),
            (
                ViewerData {
                    user_exists: true,
                    nsfw_view: Some(true),
                    age_in_years: None,
                    ..Default::default()
                },
                ViewerAge::NotStated,
                true,
            ),
            (ViewerData::default(), ViewerAge::Unknown, false),
        ] {
            let viewer = hydrator_with_viewer_data(123, data)
                .hydrate(123, SafetyLevel::FilterAll)
                .await
                .into_value();
            assert_eq!(viewer.viewer_age, expected_age);
            assert_eq!(viewer.allows_sensitive_media, expected_sensitive_media);
        }
    }

    #[test]
    fn viewer_lookup_requests_safety() {
        assert!(VIEWER_QUERY_FIELDS.contains(&QueryFields::SAFETY));
    }

    #[test]
    fn verified_badge_requires_org_type_or_blue_check() {
        let cases = [
            (Some(VerifiedType::Business), false, true),
            (Some(VerifiedType::Government), false, true),
            (None, true, true),
            (Some(VerifiedType::User), false, false),
            (Some(VerifiedType::Notable), false, false),
            (None, false, false),
        ];
        for (verified_type, is_blue_verified, expected) in cases {
            let data = ViewerData {
                verified_type,
                is_blue_verified,
                ..Default::default()
            };
            assert_eq!(
                has_verified_badge(&data),
                expected,
                "{verified_type:?} blue={is_blue_verified}"
            );
        }
    }

    #[tokio::test]
    async fn verified_badge_is_hydrated() {
        let hydrator = hydrator_with_viewer_data(
            123,
            ViewerData {
                is_blue_verified: true,
                ..Default::default()
            },
        );

        let viewer = hydrator
            .hydrate(123, SafetyLevel::FilterAll)
            .await
            .into_value();

        assert!(viewer.has_verified_badge);
    }
}
